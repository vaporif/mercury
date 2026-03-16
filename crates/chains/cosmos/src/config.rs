use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

/// Dynamic gas price configuration (e.g. osmosis txfees, skip feemarket).
#[derive(Clone, Debug, Deserialize)]
pub struct DynamicGasPrice {
    #[serde(default = "default_dynamic_gas_multiplier")]
    pub multiplier: f64,
    pub max: f64,
}

const fn default_dynamic_gas_multiplier() -> f64 {
    1.1
}

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
    /// Required when this chain hosts WASM light clients (e.g., Ethereum Beacon).
    #[serde(default)]
    pub wasm_checksum: Option<String>,
    /// When true, packet message builders use `proof_height` (0, 0) instead of the
    /// real source-chain height. This lets the dummy WASM light client's static
    /// `LatestHeight` pass the Go-level height gate in `08-wasm`.
    #[serde(default)]
    pub mock_proofs: bool,
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
        if let Some(m) = self.gas_multiplier
            && m < 1.0
        {
            eyre::bail!(
                "chain '{}': gas_multiplier must be >= 1.0, got {m}",
                self.chain_id
            );
        }
        if let Some(max) = self.max_gas
            && max == 0
        {
            eyre::bail!("chain '{}': max_gas must be > 0", self.chain_id);
        }
        if let Some(def) = self.default_gas
            && def == 0
        {
            eyre::bail!("chain '{}': default_gas must be > 0", self.chain_id);
        }
        if let Some(ref granter) = self.fee_granter
            && bech32::decode(granter).is_err()
        {
            eyre::bail!(
                "chain '{}': fee_granter is not a valid bech32 address: {granter}",
                self.chain_id
            );
        }
        if let (Some(def), Some(max)) = (self.default_gas, self.max_gas)
            && def > max
        {
            eyre::bail!(
                "chain '{}': default_gas ({def}) must be <= max_gas ({max})",
                self.chain_id
            );
        }
        if let Some(ref dgp) = self.dynamic_gas_price
            && dgp.multiplier < 1.0
        {
            eyre::bail!(
                "chain '{}': dynamic_gas_price.multiplier must be >= 1.0",
                self.chain_id
            );
        }
        if let Some(ref dgp) = self.dynamic_gas_price
            && dgp.max <= 0.0
        {
            eyre::bail!(
                "chain '{}': dynamic_gas_price.max must be > 0",
                self.chain_id
            );
        }
        if let Some(size) = self.max_tx_size
            && size == 0
        {
            eyre::bail!("chain '{}': max_tx_size must be > 0", self.chain_id);
        }
        if let Some(ref checksum) = self.wasm_checksum {
            let bytes = hex::decode(checksum).map_err(|e| {
                eyre::eyre!(
                    "chain '{}': wasm_checksum is not valid hex: {e}",
                    self.chain_id
                )
            })?;
            if bytes.len() != 32 {
                eyre::bail!(
                    "chain '{}': wasm_checksum must be 32 bytes (64 hex chars), got {} bytes",
                    self.chain_id,
                    bytes.len()
                );
            }
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
