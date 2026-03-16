use std::path::Path;

use eyre::Context;
use mercury_cosmos_counterparties::config::CosmosChainConfig;
use mercury_ethereum_counterparties::config::EthereumChainConfig;
use mercury_relay::filter::PacketFilterConfig;
use serde::Deserialize;

/// Top-level relayer configuration with chains and relay paths.
#[derive(Debug, Deserialize)]
pub struct RelayerConfig {
    #[serde(default)]
    pub chains: Vec<ChainConfig>,
    #[serde(default)]
    pub relays: Vec<RelayConfig>,
}

/// Chain backend configuration, tagged by type.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ChainConfig {
    #[serde(rename = "cosmos")]
    Cosmos(Box<CosmosChainConfig>),
    #[serde(rename = "ethereum")]
    Ethereum(EthereumChainConfig),
}

impl ChainConfig {
    /// Returns the chain identifier as a string.
    pub fn chain_id(&self) -> String {
        match self {
            Self::Cosmos(c) => c.chain_id.clone(),
            Self::Ethereum(c) => c.chain_id_str(),
        }
    }

    /// Returns the RPC endpoint address.
    pub fn rpc_addr(&self) -> &str {
        match self {
            Self::Cosmos(c) => &c.rpc_addr,
            Self::Ethereum(c) => &c.rpc_addr,
        }
    }
}

/// Defines a single relay path between two chains.
#[derive(Debug, Deserialize)]
pub struct RelayConfig {
    pub src_chain: String,
    pub dst_chain: String,
    pub src_client_id: String,
    pub dst_client_id: String,
    #[serde(default)]
    pub lookback_window_secs: Option<u64>,
    #[serde(default)]
    pub clearing_interval_secs: Option<u64>,
    #[serde(default)]
    pub misbehaviour_scan_interval_secs: Option<u64>,
    #[serde(default)]
    pub packet_filter: Option<PacketFilterConfig>,
}

/// Reads and parses a TOML relayer config from the given path.
pub fn load_config(path: &Path) -> eyre::Result<RelayerConfig> {
    let content = std::fs::read_to_string(path)
        .wrap_err_with(|| format!("reading config {}", path.display()))?;
    let config: RelayerConfig =
        toml::from_str(&content).wrap_err_with(|| format!("parsing config {}", path.display()))?;

    let mut seen_ids = std::collections::HashSet::new();
    for chain in &config.chains {
        match chain {
            ChainConfig::Cosmos(c) => c.validate()?,
            ChainConfig::Ethereum(c) => c.validate()?,
        }
        let id = chain.chain_id();
        if !seen_ids.insert(id.clone()) {
            eyre::bail!("duplicate chain_id '{id}' in config");
        }
    }
    Ok(config)
}
