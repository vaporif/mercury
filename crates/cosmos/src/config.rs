use std::time::Duration;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct CosmosChainConfig {
    pub chain_id: String,
    pub rpc_addr: String,
    pub grpc_addr: String,
    pub account_prefix: String,
    pub key_name: String,
    pub gas_price: GasPrice,
    #[serde(default = "default_block_time")]
    pub block_time: Duration,
    #[serde(default = "default_max_msg_num")]
    pub max_msg_num: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GasPrice {
    pub amount: f64,
    pub denom: String,
}

const fn default_block_time() -> Duration {
    Duration::from_secs(3)
}

const fn default_max_msg_num() -> usize {
    30
}
