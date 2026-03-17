use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use mercury_chain_cache::CachedChain;
use mercury_chain_traits::queries::ChainStatusQuery;
use mercury_chain_traits::types::ChainTypes;
use mercury_core::plugin::{self, AnyChain, ChainId, ChainPlugin, ChainStatusInfo, DynRelayConfig};
use mercury_core::registry::ChainRegistry;
use mercury_relay::context::RelayWorkerConfig;
use mercury_relay::filter::PacketFilter;

use crate::config::CosmosChainConfig;
use crate::keys::{Secp256k1KeyPair, load_cosmos_signer};
use crate::wrapper::CosmosAdapter;

type CosmosCached = CachedChain<CosmosAdapter<Secp256k1KeyPair>>;

pub fn downcast_cosmos(chain: &AnyChain) -> eyre::Result<&CosmosCached> {
    (**chain)
        .downcast_ref::<CosmosCached>()
        .ok_or_else(|| eyre::eyre!("expected cosmos chain handle"))
}

fn table_to_cosmos_config(raw: &toml::Table) -> eyre::Result<CosmosChainConfig> {
    raw.clone()
        .try_into()
        .map_err(|e| eyre::eyre!("invalid cosmos config: {e}"))
}

struct CosmosPlugin;

#[async_trait]
impl ChainPlugin for CosmosPlugin {
    fn chain_type(&self) -> &'static str {
        "cosmos"
    }

    fn validate_config(&self, raw: &toml::Table) -> eyre::Result<()> {
        let cfg = table_to_cosmos_config(raw)?;
        cfg.validate()
    }

    async fn connect(&self, raw_config: &toml::Table, config_dir: &Path) -> eyre::Result<AnyChain> {
        let cfg = table_to_cosmos_config(raw_config)?;
        let key_path = config_dir.join(&cfg.key_file);

        #[cfg(unix)]
        plugin::warn_key_file_permissions(&key_path);

        let signer = load_cosmos_signer(&key_path, &cfg.account_prefix)
            .map_err(|e| eyre::eyre!("loading signer for '{}': {e}", cfg.chain_id))?;

        let expected_chain_id = cfg.chain_id.clone();
        let chain = CosmosAdapter::new(cfg, signer)
            .await
            .map_err(|e| eyre::eyre!("connecting to '{expected_chain_id}': {e}"))?;

        let on_chain_id = chain.chain_id.to_string();
        if on_chain_id != expected_chain_id {
            eyre::bail!(
                "chain_id mismatch: config says '{expected_chain_id}', node reports '{on_chain_id}'"
            );
        }

        Ok(Arc::new(CachedChain::new(chain)) as AnyChain)
    }

    fn parse_client_id(&self, raw: &str) -> eyre::Result<plugin::AnyClientId> {
        let id: ibc::core::host::types::identifiers::ClientId = raw
            .parse()
            .map_err(|e| eyre::eyre!("invalid cosmos client ID '{raw}': {e}"))?;
        Ok(Box::new(id))
    }

    async fn query_status(&self, chain: &AnyChain) -> eyre::Result<ChainStatusInfo> {
        let c = downcast_cosmos(chain)?;
        let status = c.query_chain_status().await?;
        let height = CosmosCached::chain_status_height(&status);
        let timestamp = CosmosCached::chain_status_timestamp(&status);
        let chain_id = c.chain_id();
        Ok(ChainStatusInfo {
            chain_id: ChainId::from(chain_id.to_string()),
            height: height.value(),
            timestamp: timestamp.to_string(),
        })
    }

    fn chain_id_from_config(&self, raw: &toml::Table) -> eyre::Result<ChainId> {
        raw.get("chain_id")
            .and_then(toml::Value::as_str)
            .map(ChainId::from)
            .ok_or_else(|| eyre::eyre!("missing 'chain_id' in cosmos config"))
    }

    fn rpc_addr_from_config(&self, raw: &toml::Table) -> eyre::Result<String> {
        raw.get("rpc_addr")
            .and_then(toml::Value::as_str)
            .map(String::from)
            .ok_or_else(|| eyre::eyre!("missing 'rpc_addr' in cosmos config"))
    }
}

pub fn dyn_to_worker_config(config: &DynRelayConfig) -> eyre::Result<RelayWorkerConfig> {
    let packet_filter = config
        .packet_filter_config
        .as_ref()
        .map(|v| {
            let pfc: mercury_relay::filter::PacketFilterConfig = v
                .clone()
                .try_into()
                .map_err(|e| eyre::eyre!("invalid packet_filter config: {e}"))?;
            PacketFilter::new(&pfc)
        })
        .transpose()?;

    Ok(RelayWorkerConfig {
        lookback: config.lookback_secs.map(std::time::Duration::from_secs),
        clearing_interval: config
            .clearing_interval_secs
            .map(std::time::Duration::from_secs),
        misbehaviour_scan_interval: config
            .misbehaviour_scan_interval_secs
            .map(std::time::Duration::from_secs),
        packet_filter,
    })
}

pub fn register(registry: &mut ChainRegistry) {
    registry.register_chain(CosmosPlugin);
}
