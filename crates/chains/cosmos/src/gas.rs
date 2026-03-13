use std::sync::OnceLock;

use prost::Message;
use tracing::{debug, warn};

use crate::queries::grpc_unary;
use mercury_core::error::Result;

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

async fn query_osmosis_base_fee(channel: tonic::transport::Channel) -> Result<f64> {
    let request = tonic::Request::new(GetEipBaseFeeRequest {});
    let response = grpc_unary::<GetEipBaseFeeRequest, GetEipBaseFeeResponse>(
        channel,
        "/osmosis.txfees.v1beta1.Query/GetEipBaseFee",
        request,
    )
    .await?
    .into_inner();

    parse_decimal_price(&response.base_fee)
}

async fn query_feemarket_price(channel: tonic::transport::Channel, denom: &str) -> Result<f64> {
    let request = tonic::Request::new(GasPricesRequest {
        denom: denom.to_string(),
    });
    let response = grpc_unary::<GasPricesRequest, GasPricesResponse>(
        channel,
        "/feemarket.feemarket.v1.Query/GasPrices",
        request,
    )
    .await?
    .into_inner();

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
) -> f64 {
    let base_fee = match backend_cache.get() {
        Some(DynamicGasBackend::Osmosis) => query_osmosis_base_fee(channel).await,
        Some(DynamicGasBackend::Feemarket) => query_feemarket_price(channel, denom).await,
        Some(DynamicGasBackend::Unavailable) => {
            return static_price;
        }
        None => match query_osmosis_base_fee(channel.clone()).await {
            Ok(price) => {
                let _ = backend_cache.set(DynamicGasBackend::Osmosis);
                debug!("detected osmosis txfees backend");
                Ok(price)
            }
            Err(_) => match query_feemarket_price(channel, denom).await {
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
