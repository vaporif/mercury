use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

/// Configuration for connecting to a Cosmos SDK chain.
#[derive(Clone, Debug, Deserialize)]
pub struct CosmosChainConfig {
    pub chain_id: String,
    pub rpc_addr: String,
    pub grpc_addr: String,
    pub account_prefix: String,
    pub key_name: String,
    pub key_file: PathBuf,
    pub gas_price: GasPrice,
    #[serde(default = "default_block_time")]
    pub block_time: Duration,
    #[serde(default = "default_max_msg_num")]
    pub max_msg_num: usize,
    #[serde(default)]
    pub trusting_period: Option<Duration>,
    #[serde(default)]
    pub unbonding_period: Option<Duration>,
    #[serde(default)]
    pub max_clock_drift: Option<Duration>,
}

/// Gas price amount and denomination for fee calculation.
#[derive(Clone, Debug, Deserialize)]
pub struct GasPrice {
    pub amount: f64,
    pub denom: String,
}

impl CosmosChainConfig {
    pub fn validate(&self) -> eyre::Result<()> {
        for (name, addr) in [("rpc_addr", &self.rpc_addr), ("grpc_addr", &self.grpc_addr)] {
            if !addr.starts_with("http://") && !addr.starts_with("https://") {
                eyre::bail!(
                    "chain '{}': {name} must start with http:// or https://, got '{addr}'",
                    self.chain_id,
                );
            }
        }
        if self.gas_price.amount < 0.0 {
            eyre::bail!(
                "chain '{}': gas_price.amount must be non-negative",
                self.chain_id,
            );
        }
        Ok(())
    }
}

const fn default_block_time() -> Duration {
    Duration::from_secs(3)
}

const fn default_max_msg_num() -> usize {
    30
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> CosmosChainConfig {
        CosmosChainConfig {
            chain_id: "cosmoshub-4".to_string(),
            rpc_addr: "http://localhost:26657".to_string(),
            grpc_addr: "http://localhost:9090".to_string(),
            account_prefix: "cosmos".to_string(),
            key_name: "default".to_string(),
            key_file: "key.toml".into(),
            gas_price: GasPrice {
                amount: 0.025,
                denom: "uatom".to_string(),
            },
            block_time: default_block_time(),
            max_msg_num: default_max_msg_num(),
            trusting_period: None,
            unbonding_period: None,
            max_clock_drift: None,
        }
    }

    #[test]
    fn valid_config_passes() {
        assert!(valid_config().validate().is_ok());
    }

    #[test]
    fn https_rpc_passes() {
        let mut cfg = valid_config();
        cfg.rpc_addr = "https://rpc.cosmos.network".to_string();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn invalid_rpc_scheme_fails() {
        let mut cfg = valid_config();
        cfg.rpc_addr = "ws://localhost:26657".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn invalid_grpc_scheme_fails() {
        let mut cfg = valid_config();
        cfg.grpc_addr = "ftp://localhost:9090".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn negative_gas_price_fails() {
        let mut cfg = valid_config();
        cfg.gas_price.amount = -1.0;
        assert!(cfg.validate().is_err());
    }
}
