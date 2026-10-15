use std::sync::OnceLock;

use prost::Message;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tracing::{debug, warn};

use mercury_core::error::Result;

#[derive(Debug, Clone)]
struct ProstMessageCodec<T, U>(std::marker::PhantomData<(T, U)>);

impl<T, U> Default for ProstMessageCodec<T, U> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

#[derive(Debug, Clone)]
struct ProstMessageEncoder<T>(std::marker::PhantomData<T>);

#[derive(Debug, Clone)]
struct ProstMessageDecoder<U>(std::marker::PhantomData<U>);

impl<T: Message + Send + 'static> Encoder for ProstMessageEncoder<T> {
    type Item = T;
    type Error = tonic::Status;

    fn encode(
        &mut self,
        item: Self::Item,
        dst: &mut EncodeBuf<'_>,
    ) -> core::result::Result<(), Self::Error> {
        item.encode(dst)
            .map_err(|e| tonic::Status::internal(format!("encode error: {e}")))
    }
}

impl<U: Message + Default + Send + 'static> Decoder for ProstMessageDecoder<U> {
    type Item = U;
    type Error = tonic::Status;

    fn decode(
        &mut self,
        src: &mut DecodeBuf<'_>,
    ) -> core::result::Result<Option<Self::Item>, Self::Error> {
        let item =
            U::decode(src).map_err(|e| tonic::Status::internal(format!("decode error: {e}")))?;
        Ok(Some(item))
    }
}

impl<T, U> Codec for ProstMessageCodec<T, U>
where
    T: Message + Send + 'static,
    U: Message + Default + Send + 'static,
{
    type Encode = T;
    type Decode = U;
    type Encoder = ProstMessageEncoder<T>;
    type Decoder = ProstMessageDecoder<U>;

    fn encoder(&mut self) -> Self::Encoder {
        ProstMessageEncoder(std::marker::PhantomData)
    }

    fn decoder(&mut self) -> Self::Decoder {
        ProstMessageDecoder(std::marker::PhantomData)
    }
}

async fn grpc_unary<Req, Resp>(
    channel: tonic::transport::Channel,
    path: &str,
    request: tonic::Request<Req>,
) -> Result<Resp>
where
    Req: Message + Send + Sync + 'static,
    Resp: Message + Default + Send + Sync + 'static,
{
    let mut client = tonic::client::Grpc::new(channel);
    client
        .ready()
        .await
        .map_err(|e| eyre::eyre!("service not ready: {e}"))?;
    let path: tonic::codegen::http::uri::PathAndQuery =
        path.parse().map_err(|e| eyre::eyre!("invalid path: {e}"))?;
    let codec = ProstMessageCodec::<Req, Resp>::default();
    let response = client.unary(request, path, codec).await?;
    Ok(response.into_inner())
}

#[derive(Debug, Clone, Copy)]
pub enum DynamicGasBackend {
    Osmosis,
    Feemarket,
    Unavailable,
}

#[derive(Clone, PartialEq, Message)]
struct GetEipBaseFeeRequest {}

#[derive(Clone, PartialEq, Message)]
struct GetEipBaseFeeResponse {
    #[prost(string, tag = "1")]
    base_fee: String,
}

#[derive(Clone, PartialEq, Message)]
struct GasPricesRequest {
    #[prost(string, tag = "1")]
    denom: String,
}

#[derive(Clone, PartialEq, Message)]
struct GasPricesResponse {
    #[prost(message, repeated, tag = "1")]
    prices: Vec<DecCoin>,
}

#[derive(Clone, PartialEq, Message)]
struct DecCoin {
    #[prost(string, tag = "1")]
    denom: String,
    #[prost(string, tag = "2")]
    amount: String,
}

fn parse_decimal_price(s: &str) -> Result<f64> {
    s.parse::<f64>()
        .map_err(|e| eyre::eyre!("failed to parse gas price '{s}': {e}"))
}

fn apply_dynamic_price(base_fee: f64, multiplier: f64, max: f64) -> Result<f64> {
    eyre::ensure!(
        base_fee.is_finite() && base_fee > 0.0,
        "dynamic base fee {base_fee} is not a positive finite number"
    );
    Ok((base_fee * multiplier).min(max))
}

async fn query_osmosis_base_fee(
    channel: tonic::transport::Channel,
    rpc_guard: &mercury_core::rpc_guard::RpcGuard,
) -> Result<f64> {
    let request = tonic::Request::new(GetEipBaseFeeRequest {});
    let response = rpc_guard
        .guarded(|| async {
            grpc_unary::<GetEipBaseFeeRequest, GetEipBaseFeeResponse>(
                channel,
                "/osmosis.txfees.v1beta1.Query/GetEipBaseFee",
                request,
            )
            .await
        })
        .await?;

    parse_decimal_price(&response.base_fee)
}

async fn query_feemarket_price(
    channel: tonic::transport::Channel,
    denom: &str,
    rpc_guard: &mercury_core::rpc_guard::RpcGuard,
) -> Result<f64> {
    let request = tonic::Request::new(GasPricesRequest {
        denom: denom.to_string(),
    });
    let response = rpc_guard
        .guarded(|| async {
            grpc_unary::<GasPricesRequest, GasPricesResponse>(
                channel,
                "/feemarket.feemarket.v1.Query/GasPrices",
                request,
            )
            .await
        })
        .await?;

    let price = response
        .prices
        .first()
        .ok_or_else(|| eyre::eyre!("no gas prices returned from feemarket"))?;

    parse_decimal_price(&price.amount)
}

pub async fn resolve_gas_price(
    channel: tonic::transport::Channel,
    denom: &str,
    static_price: f64,
    dynamic_config: &crate::config::DynamicGasPrice,
    backend_cache: &OnceLock<DynamicGasBackend>,
    rpc_guard: &mercury_core::rpc_guard::RpcGuard,
) -> f64 {
    if !dynamic_config.enabled {
        return static_price;
    }

    let base_fee = match backend_cache.get() {
        Some(DynamicGasBackend::Osmosis) => query_osmosis_base_fee(channel, rpc_guard).await,
        Some(DynamicGasBackend::Feemarket) => {
            query_feemarket_price(channel, denom, rpc_guard).await
        }
        Some(DynamicGasBackend::Unavailable) => {
            return static_price;
        }
        None => match query_osmosis_base_fee(channel.clone(), rpc_guard).await {
            Ok(price) => {
                let _ = backend_cache.set(DynamicGasBackend::Osmosis);
                debug!("detected osmosis txfees backend");
                Ok(price)
            }
            Err(_) => match query_feemarket_price(channel, denom, rpc_guard).await {
                Ok(price) => {
                    let _ = backend_cache.set(DynamicGasBackend::Feemarket);
                    debug!("detected skip feemarket backend");
                    Ok(price)
                }
                Err(e) => {
                    let _ = backend_cache.set(DynamicGasBackend::Unavailable);
                    warn!(
                        "both dynamic gas backends unavailable, will use static price going forward"
                    );
                    Err(e)
                }
            },
        },
    };

    let effective = base_fee.and_then(|price| {
        let effective =
            apply_dynamic_price(price, dynamic_config.multiplier.value(), dynamic_config.max)?;
        debug!(base = price, effective, "dynamic gas price resolved");
        Ok(effective)
    });
    match effective {
        Ok(effective) => effective,
        Err(e) => {
            warn!(error = %e, "dynamic gas price unusable, using static price {static_price}");
            static_price
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_decimal_price_normal() {
        let price = parse_decimal_price("0.025").unwrap();
        assert!((price - 0.025).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_decimal_price_integer() {
        let price = parse_decimal_price("1").unwrap();
        assert!((price - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_decimal_price_empty() {
        assert!(parse_decimal_price("").is_err());
    }

    #[test]
    fn apply_dynamic_multiplier_caps_at_max() {
        let result = apply_dynamic_price(1.0, 1.5, 0.6).unwrap();
        assert!((result - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_dynamic_multiplier_normal() {
        let result = apply_dynamic_price(0.1, 1.1, 0.6).unwrap();
        assert!((result - 0.11).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_decimal_price_sdk_dec_precision() {
        let price = parse_decimal_price("0.002500000000000000").unwrap();
        assert!((price - 0.0025).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_decimal_price_rejects_garbage() {
        assert!(parse_decimal_price("abc").is_err());
        assert!(parse_decimal_price("0.025uatom").is_err());
    }

    #[test]
    fn apply_dynamic_price_rejects_non_finite_base_fee() {
        let nan = parse_decimal_price("NaN").unwrap();
        assert!(apply_dynamic_price(nan, 1.1, 0.6).is_err());
        let inf = parse_decimal_price("inf").unwrap();
        assert!(apply_dynamic_price(inf, 1.1, 0.6).is_err());
    }

    #[test]
    fn apply_dynamic_price_rejects_non_positive_base_fee() {
        assert!(apply_dynamic_price(-0.5, 1.1, 0.6).is_err());
        assert!(apply_dynamic_price(0.0, 1.1, 0.6).is_err());
    }

    #[test]
    fn apply_dynamic_price_exactly_at_max() {
        assert!((apply_dynamic_price(0.5, 1.2, 0.6).unwrap() - 0.6).abs() < 1e-12);
    }

    mod resolve {
        use super::*;
        use crate::chain::test_support::UNREACHABLE_ADDR;
        use crate::config::DynamicGasPrice;
        use mercury_core::rpc_guard::RpcGuard;
        use mercury_core::validate::GasMultiplier;

        const STATIC_PRICE: f64 = 0.025;

        fn dynamic(enabled: bool) -> DynamicGasPrice {
            DynamicGasPrice {
                multiplier: GasMultiplier::new_unchecked(1.1),
                enabled,
                max: 1.0,
            }
        }

        async fn resolve(cfg: &DynamicGasPrice, cache: &OnceLock<DynamicGasBackend>) -> f64 {
            let channel = tonic::transport::Channel::from_static(UNREACHABLE_ADDR).connect_lazy();
            resolve_gas_price(
                channel,
                "uatom",
                STATIC_PRICE,
                cfg,
                cache,
                &RpcGuard::noop("test"),
            )
            .await
        }

        fn assert_static(price: f64) {
            assert!((price - STATIC_PRICE).abs() < f64::EPSILON, "got {price}");
        }

        #[tokio::test]
        async fn disabled_returns_static_without_probing() {
            let cache = OnceLock::new();
            assert_static(resolve(&dynamic(false), &cache).await);
            assert!(
                cache.get().is_none(),
                "disabled config must not probe backends"
            );
        }

        #[tokio::test]
        async fn cached_unavailable_short_circuits_to_static() {
            let cache = OnceLock::new();
            cache.set(DynamicGasBackend::Unavailable).unwrap();
            assert_static(resolve(&dynamic(true), &cache).await);
        }

        #[tokio::test]
        async fn detection_failure_caches_unavailable_and_falls_back() {
            let cache = OnceLock::new();
            assert_static(resolve(&dynamic(true), &cache).await);
            assert!(matches!(cache.get(), Some(DynamicGasBackend::Unavailable)));
        }

        #[tokio::test]
        async fn known_backend_query_failure_falls_back_and_keeps_backend() {
            for backend in [DynamicGasBackend::Osmosis, DynamicGasBackend::Feemarket] {
                let cache = OnceLock::new();
                cache.set(backend).unwrap();
                assert_static(resolve(&dynamic(true), &cache).await);
                assert_eq!(
                    std::mem::discriminant(cache.get().unwrap()),
                    std::mem::discriminant(&backend),
                    "transient failure must not downgrade a detected backend"
                );
            }
        }
    }
}
