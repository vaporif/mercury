use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use mercury_chain_cache::CachedChain;
use mercury_chain_traits::queries::ChainStatusQuery;
use mercury_chain_traits::types::ChainTypes;
use mercury_core::plugin::{self, AnyChain, AnyClientId, ChainId, ChainPlugin, ChainStatusInfo};
use mercury_core::registry::ChainRegistry;

use crate::config::EthereumChainConfig;
use crate::keys::load_ethereum_signer;
use crate::types::EvmClientId;
use crate::wrapper::EthereumAdapter;

type EthCached = CachedChain<EthereumAdapter>;

fn downcast_eth(chain: &AnyChain) -> eyre::Result<&EthCached> {
    (**chain)
        .downcast_ref::<EthCached>()
        .ok_or_else(|| eyre::eyre!("expected ethereum chain handle"))
}

fn table_to_eth_config(raw: &toml::Table) -> eyre::Result<EthereumChainConfig> {
    raw.clone()
        .try_into()
        .map_err(|e| eyre::eyre!("invalid ethereum config: {e}"))
}

struct EthereumPlugin;

#[async_trait]
impl ChainPlugin for EthereumPlugin {
    fn chain_type(&self) -> &'static str {
        "ethereum"
    }

    fn validate_config(&self, raw: &toml::Table) -> eyre::Result<()> {
        let cfg = table_to_eth_config(raw)?;
        cfg.validate()
    }

    async fn connect(&self, raw_config: &toml::Table, config_dir: &Path) -> eyre::Result<AnyChain> {
        let cfg = table_to_eth_config(raw_config)?;
        let key_path = config_dir.join(&cfg.key_file);

        #[cfg(unix)]
        plugin::warn_key_file_permissions(&key_path);

        let expected_chain_id = cfg.chain_id;
        let signer = load_ethereum_signer(&key_path)
            .map_err(|e| eyre::eyre!("loading signer for chain {expected_chain_id}: {e}"))?;

        let chain = EthereumAdapter::new(cfg, signer)
            .await
            .map_err(|e| eyre::eyre!("connecting to chain {expected_chain_id}: {e}"))?;

        let on_chain_id = chain.chain_id().0;
        if on_chain_id != expected_chain_id {
            eyre::bail!(
                "chain_id mismatch: config says '{expected_chain_id}', node reports '{on_chain_id}'"
            );
        }

        Ok(Arc::new(CachedChain::new(chain)) as AnyChain)
    }

    fn parse_client_id(&self, raw: &str) -> eyre::Result<AnyClientId> {
        Ok(Box::new(EvmClientId(raw.to_string())))
    }

    async fn query_status(&self, chain: &AnyChain) -> eyre::Result<ChainStatusInfo> {
        let chain = downcast_eth(chain)?;
        let status = chain.query_chain_status().await?;
        let height = EthCached::chain_status_height(&status);
        let timestamp = EthCached::chain_status_timestamp(&status);
        let chain_id = chain.chain_id();
        Ok(ChainStatusInfo {
            chain_id: ChainId::from(chain_id.0.to_string()),
            height: height.0,
            timestamp: timestamp.0.to_string(),
        })
    }

    fn chain_id_from_config(&self, raw: &toml::Table) -> eyre::Result<ChainId> {
        raw.get("chain_id")
            .and_then(toml::Value::as_integer)
            .map(|id| ChainId::from(id.to_string()))
            .ok_or_else(|| eyre::eyre!("missing 'chain_id' in ethereum config"))
    }

    fn rpc_addr_from_config(&self, raw: &toml::Table) -> eyre::Result<String> {
        raw.get("rpc_addr")
            .and_then(toml::Value::as_str)
            .map(String::from)
            .ok_or_else(|| eyre::eyre!("missing 'rpc_addr' in ethereum config"))
    }
}

pub fn register(registry: &mut ChainRegistry) {
    registry.register_chain(EthereumPlugin);
}
