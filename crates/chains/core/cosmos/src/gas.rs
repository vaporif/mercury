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
struct GasPriceRequest {
    #[prost(string, tag = "1")]
    denom: String,
}

#[derive(Clone, PartialEq, Message)]
struct GasPriceResponse {
    #[prost(message, optional, tag = "1")]
    price: Option<DecCoin>,
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

const LEGACY_DEC_SCALE: f64 = 1e18;

/// `LegacyDec` fields are marshalled by gogoproto as the 10^18-scaled integer
/// (`0.03` is sent as `"30000000000000000"`). A decimal point means the value
/// is already unscaled.
fn parse_legacy_dec(s: &str) -> Result<f64> {
    if s.contains('.') {
        return parse_decimal_price(s);
    }
    let scaled: u128 = s
        .parse()
        .map_err(|e| eyre::eyre!("failed to parse gas price '{s}': {e}"))?;
    #[allow(clippy::cast_precision_loss)]
    Ok(scaled as f64 / LEGACY_DEC_SCALE)
}

/// The node answered but has no price for our denom.
#[derive(Debug)]
struct NoGasPrice(String);

impl std::fmt::Display for NoGasPrice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "feemarket returned no gas price for denom '{}'", self.0)
    }
}

impl std::error::Error for NoGasPrice {}

/// Whether a probe failure says the backend is absent, as opposed to a
/// transient error (timeout, connection reset) worth retrying later.
fn backend_absent(e: &eyre::Report) -> bool {
    e.downcast_ref::<NoGasPrice>().is_some()
        || e.chain().any(|c| {
            c.downcast_ref::<tonic::Status>()
                .is_some_and(|s| s.code() == tonic::Code::Unimplemented)
        })
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

    parse_legacy_dec(&response.base_fee)
}

async fn query_feemarket_price(
    channel: tonic::transport::Channel,
    denom: &str,
    rpc_guard: &mercury_core::rpc_guard::RpcGuard,
) -> Result<f64> {
    let request = tonic::Request::new(GasPriceRequest {
        denom: denom.to_string(),
    });
    let response = rpc_guard
        .guarded(|| async {
            grpc_unary::<GasPriceRequest, GasPriceResponse>(
                channel,
                "/feemarket.feemarket.v1.Query/GasPrice",
                request,
            )
            .await
        })
        .await?;

    let price = response
        .price
        .filter(|p| p.denom == denom)
        .ok_or_else(|| NoGasPrice(denom.to_string()))?;

    parse_legacy_dec(&price.amount)
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
            Err(osmosis_err) => match query_feemarket_price(channel, denom, rpc_guard).await {
                Ok(price) => {
                    let _ = backend_cache.set(DynamicGasBackend::Feemarket);
                    debug!("detected skip feemarket backend");
                    Ok(price)
                }
                Err(e) if backend_absent(&osmosis_err) && backend_absent(&e) => {
                    let _ = backend_cache.set(DynamicGasBackend::Unavailable);
                    warn!(
                        "both dynamic gas backends unavailable, will use static price going forward"
                    );
                    Err(e)
                }
                // Transient: leave the cache empty so the next resolution re-probes.
                Err(e) => Err(e),
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
        async fn unreachable_node_falls_back_without_caching_a_backend() {
            let cache = OnceLock::new();
            assert_static(resolve(&dynamic(true), &cache).await);
            assert!(cache.get().is_none(), "transport errors are retried later");
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

    mod mocked {
        use super::*;
        use crate::chain::test_support::grpc::{
            GrpcReply, MockGrpc, reply, spawn_grpc, unimplemented,
        };
        use crate::config::DynamicGasPrice;
        use mercury_core::rpc_guard::RpcGuard;
        use mercury_core::validate::GasMultiplier;

        const STATIC_PRICE: f64 = 0.025;
        const OSMOSIS: &str = "/osmosis.txfees.v1beta1.Query/GetEipBaseFee";
        const FEEMARKET: &str = "/feemarket.feemarket.v1.Query/GasPrice";
        const FEEMARKET_ALL: &str = "/feemarket.feemarket.v1.Query/GasPrices";

        fn cfg() -> DynamicGasPrice {
            DynamicGasPrice {
                multiplier: GasMultiplier::new_unchecked(1.1),
                enabled: true,
                max: 1.0,
            }
        }

        fn osmosis(base_fee: &str) -> GrpcReply {
            reply(&GetEipBaseFeeResponse {
                base_fee: base_fee.into(),
            })
        }

        fn feemarket(denom: &str, amount: &str) -> GrpcReply {
            reply(&GasPriceResponse {
                price: Some(DecCoin {
                    denom: denom.into(),
                    amount: amount.into(),
                }),
            })
        }

        // GasPrices (all denoms) response: `repeated DecCoin prices = 1`.
        #[derive(Clone, PartialEq, Message)]
        struct GasPricesResponse {
            #[prost(message, repeated, tag = "1")]
            prices: Vec<DecCoin>,
        }

        async fn resolve(grpc: &MockGrpc, cache: &OnceLock<DynamicGasBackend>) -> f64 {
            let channel = tonic::transport::Channel::from_shared(grpc.url.clone())
                .unwrap()
                .connect_lazy();
            resolve_gas_price(
                channel,
                "uatom",
                STATIC_PRICE,
                &cfg(),
                cache,
                &RpcGuard::noop("test"),
            )
            .await
        }

        fn assert_price(got: f64, want: f64) {
            assert!((got - want).abs() < 1e-12, "got {got}, want {want}");
        }

        #[tokio::test]
        async fn detects_osmosis_and_applies_multiplier() {
            let grpc = spawn_grpc(|call| match call.path.as_str() {
                OSMOSIS => osmosis("0.0025"),
                _ => unimplemented(call),
            })
            .await;
            let cache = OnceLock::new();
            assert_price(resolve(&grpc, &cache).await, 0.0025 * 1.1);
            assert!(matches!(cache.get(), Some(DynamicGasBackend::Osmosis)));

            // cached backend: osmosis only, never probes feemarket
            assert_price(resolve(&grpc, &cache).await, 0.0025 * 1.1);
            assert_eq!(grpc.calls_to(OSMOSIS).len(), 2);
            assert!(grpc.calls_to(FEEMARKET).is_empty());
        }

        #[tokio::test]
        async fn falls_back_to_feemarket_when_osmosis_unimplemented() {
            let grpc = spawn_grpc(|call| match call.path.as_str() {
                FEEMARKET => feemarket("uatom", "0.5"),
                _ => unimplemented(call),
            })
            .await;
            let cache = OnceLock::new();
            assert_price(resolve(&grpc, &cache).await, 0.5 * 1.1);
            assert!(matches!(cache.get(), Some(DynamicGasBackend::Feemarket)));
            let req: GasPriceRequest = grpc.calls_to(FEEMARKET)[0].decode();
            assert_eq!(req.denom, "uatom");

            resolve(&grpc, &cache).await;
            assert_eq!(
                grpc.calls_to(OSMOSIS).len(),
                1,
                "osmosis probed only during detection"
            );
        }

        #[tokio::test]
        async fn dynamic_price_capped_at_max() {
            let grpc = spawn_grpc(|_| osmosis("5.0")).await;
            assert_price(resolve(&grpc, &OnceLock::new()).await, 1.0);
        }

        #[tokio::test]
        async fn feemarket_without_price_for_denom_marks_backends_unavailable() {
            let grpc = spawn_grpc(|call| match call.path.as_str() {
                FEEMARKET => reply(&GasPriceResponse { price: None }),
                _ => unimplemented(call),
            })
            .await;
            let cache = OnceLock::new();
            assert_price(resolve(&grpc, &cache).await, STATIC_PRICE);
            assert!(matches!(cache.get(), Some(DynamicGasBackend::Unavailable)));

            assert_price(resolve(&grpc, &cache).await, STATIC_PRICE);
            assert_eq!(
                grpc.calls().len(),
                2,
                "unavailable backend is never re-probed"
            );
        }

        #[tokio::test]
        async fn unusable_base_fee_falls_back_to_static_and_keeps_backend() {
            for bad in ["not-a-number", "0", "-1"] {
                let grpc = spawn_grpc(move |_| osmosis(bad)).await;
                let cache = OnceLock::new();
                cache.set(DynamicGasBackend::Osmosis).unwrap();
                assert_price(resolve(&grpc, &cache).await, STATIC_PRICE);
                assert!(matches!(cache.get(), Some(DynamicGasBackend::Osmosis)));
            }
        }

        // Response bytes captured from mainnet via CometBFT abci_query (same
        // gogoproto marshalling the gRPC server uses). LegacyDec fields go on the
        // wire as the 10^18-scaled integer, not a decimal string.
        // osmosis-1 h=72582341 GetEipBaseFee: base_fee = 0.03
        const OSMOSIS_MAINNET_BASE_FEE: &[u8] = b"\n\x1130000000000000000";
        // cosmoshub-4 h=33423002 GasPrices: uatom = 0.005
        const HUB_MAINNET_GAS_PRICES: &[u8] = b"\n\x19\n\x05uatom\x12\x105000000000000000";

        #[tokio::test]
        async fn osmosis_base_fee_decodes_legacy_dec_wire_format() {
            let grpc = spawn_grpc(|call| match call.path.as_str() {
                OSMOSIS => GrpcReply::Ok(OSMOSIS_MAINNET_BASE_FEE.to_vec()),
                _ => unimplemented(call),
            })
            .await;
            assert_price(resolve(&grpc, &OnceLock::new()).await, 0.03 * 1.1);
        }

        #[tokio::test]
        async fn feemarket_price_decodes_legacy_dec_wire_format() {
            let grpc = spawn_grpc(|call| match call.path.as_str() {
                FEEMARKET => GrpcReply::Ok(HUB_MAINNET_GAS_PRICES.to_vec()),
                _ => unimplemented(call),
            })
            .await;
            assert_price(resolve(&grpc, &OnceLock::new()).await, 0.005 * 1.1);
        }

        #[tokio::test]
        async fn feemarket_price_is_for_configured_denom() {
            // A real feemarket node: GasPricesRequest has no fields, so the denom
            // mercury sends is an unknown field and every denom comes back,
            // sorted (neutron-1 lists 23, `factory/...` first). GasPrice is the
            // per-denom RPC.
            let grpc = spawn_grpc(|call| match call.path.as_str() {
                FEEMARKET_ALL => reply(&GasPricesResponse {
                    prices: [("factory/x/APOLLO", "0.9"), ("uatom", "0.5")]
                        .into_iter()
                        .map(|(denom, amount)| DecCoin {
                            denom: denom.into(),
                            amount: amount.into(),
                        })
                        .collect(),
                }),
                FEEMARKET => feemarket("uatom", "0.5"),
                _ => unimplemented(call),
            })
            .await;
            assert_price(resolve(&grpc, &OnceLock::new()).await, 0.5 * 1.1);
        }

        #[tokio::test]
        async fn transient_probe_failure_does_not_disable_dynamic_pricing() {
            let feemarket_calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let counter = feemarket_calls.clone();
            let grpc = spawn_grpc(move |call| match call.path.as_str() {
                FEEMARKET => {
                    if counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        GrpcReply::Err(tonic::Status::unavailable("node restarting"))
                    } else {
                        feemarket("uatom", "0.5")
                    }
                }
                _ => unimplemented(call),
            })
            .await;
            let cache = OnceLock::new();
            // first resolution hits the transient error: static fallback is fine
            assert_price(resolve(&grpc, &cache).await, STATIC_PRICE);
            // later resolutions must pick the backend up once it answers
            let mut later = Vec::new();
            for _ in 0..3 {
                later.push(resolve(&grpc, &cache).await);
            }
            assert!(
                later.iter().any(|p| (p - 0.5 * 1.1).abs() < 1e-12),
                "prices after recovery {later:?}, cache {:?}, feemarket calls {}",
                cache.get(),
                grpc.calls_to(FEEMARKET).len()
            );
        }

        #[tokio::test]
        async fn cached_feemarket_queries_only_feemarket() {
            let grpc = spawn_grpc(|call| match call.path.as_str() {
                FEEMARKET => feemarket("uatom", "0.2"),
                _ => osmosis("0.9"),
            })
            .await;
            let cache = OnceLock::new();
            cache.set(DynamicGasBackend::Feemarket).unwrap();
            assert_price(resolve(&grpc, &cache).await, 0.2 * 1.1);
            assert!(grpc.calls_to(OSMOSIS).is_empty());
        }
    }
}
