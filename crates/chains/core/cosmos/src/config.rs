use std::path::PathBuf;
use std::time::Duration;

use eyre::WrapErr;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct DynamicGasPrice {
    #[serde(default = "default_dynamic_gas_multiplier")]
    pub multiplier: f64,
    pub max: f64,
}

const fn default_dynamic_gas_multiplier() -> f64 {
    1.1
}

#[derive(Clone, Debug, Deserialize)]
pub struct CosmosChainConfig {
    #[serde(default)]
    pub chain_name: Option<String>,
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
    #[serde(default)]
    pub gas_multiplier: Option<f64>,
    #[serde(default)]
    pub max_gas: Option<u64>,
    #[serde(default)]
    pub default_gas: Option<u64>,
    #[serde(default)]
    pub fee_granter: Option<String>,
    #[serde(default)]
    pub dynamic_gas_price: Option<DynamicGasPrice>,
    #[serde(default)]
    pub max_tx_size: Option<usize>,
    /// SHA-256 checksum of the WASM light client module (hex-encoded, 32 bytes).
    #[serde(default)]
    pub wasm_checksum: Option<String>,
    /// When true, packet message builders use `proof_height` (0, 0) instead of the
    /// real source-chain height. This lets the dummy WASM light client's static
    /// `LatestHeight` pass the Go-level height gate in `08-wasm`.
    #[serde(default)]
    pub mock_proofs: bool,
    #[serde(default = "mercury_core::rpc_guard::default_timeout_secs")]
    pub rpc_timeout_secs: u64,
    #[serde(default = "mercury_core::rpc_guard::default_rate_limit")]
    pub rpc_rate_limit: u64,
}

/// Gas price amount and denomination for fee calculation.
#[derive(Clone, Debug, Deserialize)]
pub struct GasPrice {
    pub amount: f64,
    pub denom: String,
}

impl CosmosChainConfig {
    #[must_use]
    pub const fn rpc_config(&self) -> mercury_core::rpc_guard::RpcConfig {
        mercury_core::rpc_guard::RpcConfig {
            rpc_timeout: Duration::from_secs(self.rpc_timeout_secs),
            rate_limit: self.rpc_rate_limit,
        }
    }

    pub fn validate(&self) -> eyre::Result<()> {
        self.validate_inner()
            .wrap_err_with(|| format!("chain '{}'", self.chain_id))
    }

    fn validate_inner(&self) -> eyre::Result<()> {
        use mercury_core::validate::{require_http_url, require_positive};

        require_http_url("rpc_addr", &self.rpc_addr)?;
        require_http_url("grpc_addr", &self.grpc_addr)?;
        eyre::ensure!(
            self.gas_price.amount >= 0.0,
            "gas_price.amount must be non-negative"
        );
        if let Some(m) = self.gas_multiplier {
            eyre::ensure!(m >= 1.0, "gas_multiplier must be >= 1.0, got {m}");
        }
        if let Some(max) = self.max_gas {
            require_positive("max_gas", &max)?;
        }
        if let Some(def) = self.default_gas {
            require_positive("default_gas", &def)?;
        }
        if let Some(ref granter) = self.fee_granter {
            eyre::ensure!(
                bech32::decode(granter).is_ok(),
                "fee_granter is not a valid bech32 address: {granter}"
            );
        }
        if let (Some(def), Some(max)) = (self.default_gas, self.max_gas) {
            eyre::ensure!(def <= max, "default_gas ({def}) must be <= max_gas ({max})");
        }
        if let Some(ref dgp) = self.dynamic_gas_price {
            eyre::ensure!(
                dgp.multiplier >= 1.0,
                "dynamic_gas_price.multiplier must be >= 1.0"
            );
            require_positive("dynamic_gas_price.max", &dgp.max)?;
        }
        if let Some(size) = self.max_tx_size {
            require_positive("max_tx_size", &size)?;
        }
        if let (Some(trusting), Some(unbonding)) = (self.trusting_period, self.unbonding_period) {
            eyre::ensure!(
                trusting < unbonding,
                "trusting_period ({trusting:?}) must be less than unbonding_period ({unbonding:?})"
            );
        }
        if let Some(ref checksum) = self.wasm_checksum {
            let bytes = hex::decode(checksum)
                .map_err(|e| eyre::eyre!("wasm_checksum is not valid hex: {e}"))?;
            eyre::ensure!(
                bytes.len() == 32,
                "wasm_checksum must be 32 bytes (64 hex chars), got {} bytes",
                bytes.len()
            );
        }
        Ok(())
    }
}

pub(crate) const DEFAULT_MAX_TX_SIZE: usize = 180_000;

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
            chain_name: None,
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
            gas_multiplier: None,
            max_gas: None,
            default_gas: None,
            fee_granter: None,
            dynamic_gas_price: None,
            max_tx_size: None,
            wasm_checksum: None,
            mock_proofs: false,
            rpc_timeout_secs: mercury_core::rpc_guard::default_timeout_secs(),
            rpc_rate_limit: mercury_core::rpc_guard::default_rate_limit(),
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

    #[test]
    fn gas_multiplier_below_min_fails() {
        let mut cfg = valid_config();
        cfg.gas_multiplier = Some(0.9);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn gas_multiplier_at_min_passes() {
        let mut cfg = valid_config();
        cfg.gas_multiplier = Some(1.0);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn max_gas_zero_fails() {
        let mut cfg = valid_config();
        cfg.max_gas = Some(0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn default_gas_exceeds_max_gas_fails() {
        let mut cfg = valid_config();
        cfg.max_gas = Some(100_000);
        cfg.default_gas = Some(200_000);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn dynamic_gas_price_multiplier_below_min_fails() {
        let mut cfg = valid_config();
        cfg.dynamic_gas_price = Some(DynamicGasPrice {
            multiplier: 0.5,
            max: 0.6,
        });
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn max_tx_size_zero_fails() {
        let mut cfg = valid_config();
        cfg.max_tx_size = Some(0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn fee_granter_invalid_bech32_fails() {
        let mut cfg = valid_config();
        cfg.fee_granter = Some("notabech32address".to_string());
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn default_gas_without_max_gas_passes() {
        let mut cfg = valid_config();
        cfg.default_gas = Some(500_000);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn valid_wasm_checksum_passes() {
        let mut cfg = valid_config();
        cfg.wasm_checksum = Some("a".repeat(64));
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn invalid_wasm_checksum_hex_fails() {
        let mut cfg = valid_config();
        cfg.wasm_checksum = Some("not_hex".to_string());
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn wasm_checksum_wrong_length_fails() {
        let mut cfg = valid_config();
        cfg.wasm_checksum = Some("aabb".to_string());
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn trusting_period_exceeds_unbonding_fails() {
        let mut cfg = valid_config();
        cfg.trusting_period = Some(Duration::from_secs(86400 * 21));
        cfg.unbonding_period = Some(Duration::from_secs(86400 * 14));
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn trusting_period_equals_unbonding_fails() {
        let mut cfg = valid_config();
        cfg.trusting_period = Some(Duration::from_secs(86400 * 14));
        cfg.unbonding_period = Some(Duration::from_secs(86400 * 14));
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn trusting_period_less_than_unbonding_passes() {
        let mut cfg = valid_config();
        cfg.trusting_period = Some(Duration::from_secs(86400 * 14));
        cfg.unbonding_period = Some(Duration::from_secs(86400 * 21));
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn rpc_config_defaults_from_toml() {
        let toml_str = r#"
            chain_id = "test-1"
            rpc_addr = "http://localhost:26657"
            grpc_addr = "http://localhost:9090"
            account_prefix = "cosmos"
            key_name = "default"
            key_file = "key.toml"
            [gas_price]
            amount = 0.025
            denom = "uatom"
        "#;
        let config: CosmosChainConfig = toml::from_str(toml_str).unwrap();
        let rpc_config = config.rpc_config();
        assert_eq!(
            rpc_config.rpc_timeout,
            Duration::from_secs(mercury_core::rpc_guard::RpcConfig::DEFAULT_TIMEOUT_SECS)
        );
        assert_eq!(
            rpc_config.rate_limit,
            mercury_core::rpc_guard::RpcConfig::DEFAULT_RATE_LIMIT
        );
    }

    #[test]
    fn rpc_config_custom_from_toml() {
        let toml_str = r#"
            chain_id = "test-1"
            rpc_addr = "http://localhost:26657"
            grpc_addr = "http://localhost:9090"
            account_prefix = "cosmos"
            key_name = "default"
            key_file = "key.toml"
            rpc_timeout_secs = 60
            rpc_rate_limit = 50
            [gas_price]
            amount = 0.025
            denom = "uatom"
        "#;
        let config: CosmosChainConfig = toml::from_str(toml_str).unwrap();
        let rpc_config = config.rpc_config();
        assert_eq!(rpc_config.rpc_timeout, Duration::from_secs(60));
        assert_eq!(rpc_config.rate_limit, 50);
    }

    #[test]
    fn rpc_config_zero_rate_limit_rejected() {
        let toml_str = r#"
            chain_id = "test-1"
            rpc_addr = "http://localhost:26657"
            grpc_addr = "http://localhost:9090"
            account_prefix = "cosmos"
            key_name = "default"
            key_file = "key.toml"
            rpc_rate_limit = 0
            [gas_price]
            amount = 0.025
            denom = "uatom"
        "#;
        let config: CosmosChainConfig = toml::from_str(toml_str).unwrap();
        assert!(config.rpc_config().validate().is_err());
    }

    #[test]
    fn trusting_period_without_unbonding_passes() {
        let mut cfg = valid_config();
        cfg.trusting_period = Some(Duration::from_secs(86400 * 14));
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn valid_gas_config_passes() {
        let mut cfg = valid_config();
        cfg.gas_multiplier = Some(1.1);
        cfg.max_gas = Some(400_000);
        cfg.default_gas = Some(300_000);
        cfg.fee_granter = Some("cosmos1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5lzv7xu".to_string());
        cfg.max_tx_size = Some(180_000);
        cfg.dynamic_gas_price = Some(DynamicGasPrice {
            multiplier: 1.1,
            max: 0.6,
        });
        assert!(cfg.validate().is_ok());
    }
}
