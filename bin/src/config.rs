use std::path::Path;

use eyre::Context;
use mercury_core::plugin::ChainId;
use mercury_core::registry::ChainRegistry;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct RelayerConfig {
    #[serde(default)]
    pub chains: Vec<RawChainConfig>,
    #[serde(default)]
    pub relays: Vec<RelayConfig>,
    #[serde(default)]
    pub telemetry: mercury_telemetry::TelemetryConfig,
}

impl RelayerConfig {
    pub fn find_chain<'a>(
        &'a self,
        registry: &ChainRegistry,
        chain_id: &str,
    ) -> eyre::Result<&'a RawChainConfig> {
        self.chains
            .iter()
            .find(|c| c.chain_id(registry).is_ok_and(|id| id.as_ref() == chain_id))
            .ok_or_else(|| eyre::eyre!("chain '{chain_id}' not found in config"))
    }
}

/// `raw` will contain the `type` key — plugins should tolerate that.
#[derive(Debug, Deserialize)]
pub struct RawChainConfig {
    #[serde(rename = "type")]
    pub chain_type: String,
    #[serde(flatten)]
    pub raw: toml::Table,
}

impl RawChainConfig {
    pub fn chain_id(&self, registry: &ChainRegistry) -> eyre::Result<ChainId> {
        let plugin = registry.chain(&self.chain_type)?;
        plugin.chain_id_from_config(&self.raw)
    }
}

#[derive(Debug, Deserialize)]
pub struct RelayConfig {
    pub src_chain: String,
    pub dst_chain: String,
    pub src_client_id: String,
    pub dst_client_id: String,
    #[serde(default)]
    pub lookback_window_secs: Option<u64>,
    #[serde(default)]
    pub sweep_interval_secs: Option<u64>,
    #[serde(default)]
    pub misbehaviour_scan_interval_secs: Option<u64>,
    #[serde(default)]
    pub packet_filter: Option<toml::Value>,
    #[serde(default = "default_true")]
    pub clear_on_start: bool,
    #[serde(default = "default_clear_limit")]
    pub clear_limit: Option<usize>,
    #[serde(default)]
    pub excluded_sequences: Vec<u64>,
}

fn default_true() -> bool {
    true
}

fn default_clear_limit() -> Option<usize> {
    Some(50)
}

pub fn load_config(path: &Path, registry: &ChainRegistry) -> eyre::Result<RelayerConfig> {
    let content = std::fs::read_to_string(path)
        .wrap_err_with(|| format!("reading config {}", path.display()))?;
    let config: RelayerConfig =
        toml::from_str(&content).wrap_err_with(|| format!("parsing config {}", path.display()))?;

    let mut seen_ids = std::collections::HashSet::new();
    for chain in &config.chains {
        let plugin = registry.chain(&chain.chain_type)?;
        plugin.validate_config(&chain.raw)?;

        let id = plugin.chain_id_from_config(&chain.raw)?;
        if !seen_ids.insert(id.clone()) {
            eyre::bail!("duplicate chain_id '{id}' in config");
        }
    }
    Ok(config)
}
