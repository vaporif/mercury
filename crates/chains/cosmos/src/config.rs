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
