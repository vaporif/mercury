use std::path::Path;

use eyre::Context;
use mercury_cosmos::config::CosmosChainConfig;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct RelayerConfig {
    #[serde(default)]
    pub chains: Vec<ChainConfig>,
    #[serde(default)]
    pub relays: Vec<RelayConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ChainConfig {
    #[serde(rename = "cosmos")]
    Cosmos(CosmosChainConfig),
}

impl ChainConfig {
    pub fn chain_id(&self) -> &str {
        match self {
            Self::Cosmos(c) => &c.chain_id,
        }
    }

    pub fn rpc_addr(&self) -> &str {
        match self {
            Self::Cosmos(c) => &c.rpc_addr,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RelayConfig {
    pub src_chain: String,
    pub dst_chain: String,
    pub src_client_id: String,
    pub dst_client_id: String,
}

pub fn load_config(path: &Path) -> eyre::Result<RelayerConfig> {
    let content = std::fs::read_to_string(path)
        .wrap_err_with(|| format!("reading config {}", path.display()))?;
    let config: RelayerConfig =
        toml::from_str(&content).wrap_err_with(|| format!("parsing config {}", path.display()))?;
    Ok(config)
}
