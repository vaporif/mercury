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
) -> core::result::Result<tonic::Response<Resp>, tonic::Status>
where
    Req: Message + Send + Sync + 'static,
    Resp: Message + Default + Send + Sync + 'static,
{
    let mut client = tonic::client::Grpc::new(channel);
    client
        .ready()
        .await
        .map_err(|e| tonic::Status::unknown(format!("service not ready: {e}")))?;
    let path: tonic::codegen::http::uri::PathAndQuery = path
        .parse()
        .map_err(|e| tonic::Status::internal(format!("invalid path: {e}")))?;
    let codec = ProstMessageCodec::<Req, Resp>::default();
    client.unary(request, path, codec).await
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

fn apply_dynamic_price(base_fee: f64, multiplier: f64, max: f64) -> f64 {
    let adjusted = base_fee * multiplier;
    adjusted.min(max)
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
            .map(|r| r.into_inner())
            .map_err(Into::into)
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
            .map(|r| r.into_inner())
            .map_err(Into::into)
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
    let base_fee = match backend_cache.get() {
        Some(DynamicGasBackend::Osmosis) => {
            query_osmosis_base_fee(channel, rpc_guard).await
        }
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

    match base_fee {
        Ok(price) => {
            let effective =
                apply_dynamic_price(price, dynamic_config.multiplier, dynamic_config.max);
            debug!(base = price, effective, "dynamic gas price resolved");
            effective
        }
        Err(e) => {
            warn!(error = %e, "dynamic gas price query failed, using static price {static_price}");
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
        let result = apply_dynamic_price(1.0, 1.5, 0.6);
        assert!((result - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_dynamic_multiplier_normal() {
        let result = apply_dynamic_price(0.1, 1.1, 0.6);
        assert!((result - 0.11).abs() < f64::EPSILON);
    }
}
