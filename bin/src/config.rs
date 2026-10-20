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
    pub clear_limit: usize,
    #[serde(default)]
    pub excluded_sequences: Vec<u64>,
}

const fn default_true() -> bool {
    true
}

const fn default_clear_limit() -> usize {
    50
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::build_registry;
    use crate::test_support::{
        EXAMPLE_CONFIG, HUB, OSMO, cosmos_chain, err_string, missing_path, write_config,
    };

    #[test]
    fn example_config_parses_and_validates() {
        let registry = build_registry();
        let cfg = load_config(Path::new(EXAMPLE_CONFIG), &registry).unwrap();
        assert_eq!(cfg.chains.len(), 2);
        assert_eq!(cfg.relays.len(), 1);
        assert!(cfg.telemetry.otlp_endpoint.is_none());

        let relay = &cfg.relays[0];
        assert_eq!(
            (relay.src_chain.as_str(), relay.dst_chain.as_str()),
            (HUB, OSMO)
        );
        assert_eq!(relay.src_client_id, "07-tendermint-0");
        // Optional relay fields left out of the example take their defaults.
        assert!(relay.clear_on_start);
        assert_eq!(relay.clear_limit, 50);
        assert_eq!(relay.excluded_sequences, Vec::<u64>::new());
        assert!(relay.packet_filter.is_none());
        assert!(relay.lookback_window_secs.is_none());

        assert_eq!(
            cfg.find_chain(&registry, OSMO).unwrap().chain_type,
            "cosmos"
        );
    }

    #[test]
    fn raw_chain_table_omits_type_key_contrary_to_doc_comment() {
        // RawChainConfig's doc says `raw` contains `type`; serde's flatten
        // hands the renamed field to `chain_type` instead. Pins the real shape.
        let registry = build_registry();
        let cfg = load_config(Path::new(EXAMPLE_CONFIG), &registry).unwrap();
        let chain = cfg.find_chain(&registry, HUB).unwrap();
        assert_eq!(chain.chain_type, "cosmos");
        assert_eq!(chain.raw.get("type"), None);
        assert_eq!(
            chain.raw.get("chain_id").and_then(toml::Value::as_str),
            Some(HUB)
        );
    }

    #[test]
    fn empty_config_is_valid_with_defaults() {
        let file = write_config("");
        let cfg = load_config(&file.path, &build_registry()).unwrap();
        assert!(cfg.chains.is_empty());
        assert!(cfg.relays.is_empty());
    }

    #[test]
    fn relay_overrides_are_parsed() {
        let file = write_config(
            r#"
[[relays]]
src_chain = "a"
dst_chain = "b"
src_client_id = "x"
dst_client_id = "y"
lookback_window_secs = 600
sweep_interval_secs = 30
misbehaviour_scan_interval_secs = 120
clear_on_start = false
clear_limit = 5
excluded_sequences = [3, 7]

[relays.packet_filter]
policy = "allow"
source_ports = ["transfer"]

[telemetry]
otlp_endpoint = "http://localhost:4318"
"#,
        );
        let cfg = load_config(&file.path, &build_registry()).unwrap();
        let relay = &cfg.relays[0];
        assert_eq!(relay.lookback_window_secs, Some(600));
        assert_eq!(relay.sweep_interval_secs, Some(30));
        assert_eq!(relay.misbehaviour_scan_interval_secs, Some(120));
        assert!(!relay.clear_on_start);
        assert_eq!(relay.clear_limit, 5);
        assert_eq!(relay.excluded_sequences, vec![3, 7]);
        let filter = relay.packet_filter.as_ref().unwrap();
        assert_eq!(
            filter.get("policy").and_then(toml::Value::as_str),
            Some("allow")
        );
        assert_eq!(
            cfg.telemetry.otlp_endpoint.as_deref(),
            Some("http://localhost:4318")
        );
    }

    #[test]
    fn missing_file_error_names_the_path() {
        let path = missing_path();
        let err = err_string(load_config(&path, &build_registry()));
        assert!(
            err.starts_with(&format!("reading config {}", path.display())),
            "{err}"
        );
    }

    #[test]
    fn malformed_toml_is_rejected() {
        let file = write_config("[[chains]\ntype = ");
        let err = err_string(load_config(&file.path, &build_registry()));
        assert!(err.starts_with("parsing config"), "{err}");
    }

    #[test]
    fn chain_without_type_is_rejected() {
        let file = write_config("[[chains]]\nchain_id = \"a\"\n");
        let err = err_string(load_config(&file.path, &build_registry()));
        assert!(err.contains("missing field `type`"), "{err}");
    }

    #[test]
    fn relay_missing_client_id_is_rejected() {
        let file = write_config(
            "[[relays]]\nsrc_chain = \"a\"\ndst_chain = \"b\"\nsrc_client_id = \"x\"\n",
        );
        let err = err_string(load_config(&file.path, &build_registry()));
        assert!(err.contains("missing field `dst_client_id`"), "{err}");
    }

    #[test]
    fn unknown_chain_type_is_rejected() {
        let file = write_config("[[chains]]\ntype = \"near\"\nchain_id = \"a\"\n");
        let err = err_string(load_config(&file.path, &build_registry()));
        assert_eq!(err, "unsupported chain type 'near'");
    }

    #[test]
    fn plugin_validation_runs_for_each_chain() {
        let bad = cosmos_chain(HUB).replace("http://127.0.0.1:1", "ftp://127.0.0.1:1");
        let file = write_config(&bad);
        assert!(load_config(&file.path, &build_registry()).is_err());
    }

    #[test]
    fn duplicate_chain_id_is_rejected() {
        let file = write_config(&format!("{}{}", cosmos_chain(HUB), cosmos_chain(HUB)));
        let err = err_string(load_config(&file.path, &build_registry()));
        assert_eq!(err, format!("duplicate chain_id '{HUB}' in config"));
    }

    #[test]
    fn find_chain_reports_unknown_chain_id() {
        let registry = build_registry();
        let cfg = load_config(Path::new(EXAMPLE_CONFIG), &registry).unwrap();
        let err = err_string(cfg.find_chain(&registry, "juno-1"));
        assert_eq!(err, "chain 'juno-1' not found in config");
    }

    #[test]
    fn find_chain_skips_chains_of_unregistered_type() {
        // find_chain runs on unvalidated configs too; an unknown type must not
        // abort the search for other chains.
        let file = write_config(&format!(
            "[[chains]]\ntype = \"near\"\nchain_id = \"a\"\n{}",
            cosmos_chain(OSMO)
        ));
        let cfg: RelayerConfig =
            toml::from_str(&std::fs::read_to_string(&file.path).unwrap()).unwrap();
        let registry = build_registry();
        assert_eq!(
            cfg.find_chain(&registry, OSMO).unwrap().chain_type,
            "cosmos"
        );
        assert!(cfg.find_chain(&registry, "a").is_err());
        assert!(cfg.chains[0].chain_id(&registry).is_err());
    }
}
