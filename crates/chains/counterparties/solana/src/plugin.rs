use std::any::Any;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use mercury_chain_cache::CachedChain;
use mercury_chain_traits::queries::ChainStatusQuery;
use mercury_chain_traits::types::ChainTypes;
use mercury_core::plugin::{self, AnyChain, ChainId, ChainPlugin, ChainStatusInfo};
use mercury_core::registry::ChainRegistry;

use mercury_solana::config::SolanaChainConfig;
use mercury_solana::types::SolanaClientId;

use crate::wrapper::SolanaAdapter;

type SolanaCached = CachedChain<SolanaAdapter>;

pub fn downcast_solana(chain: &AnyChain) -> eyre::Result<&SolanaCached> {
    (**chain)
        .downcast_ref::<SolanaCached>()
        .ok_or_else(|| eyre::eyre!("expected solana chain handle"))
}

fn table_to_solana_config(raw: &toml::Table) -> eyre::Result<SolanaChainConfig> {
    raw.clone()
        .try_into()
        .map_err(|e| eyre::eyre!("invalid solana config: {e}"))
}

struct SolanaPlugin;

#[async_trait]
impl ChainPlugin for SolanaPlugin {
    fn chain_type(&self) -> &'static str {
        "solana"
    }

    fn validate_config(&self, raw: &toml::Table) -> eyre::Result<()> {
        let cfg = table_to_solana_config(raw)?;
        cfg.validate()
    }

    async fn connect(
        &self,
        raw_config: &toml::Table,
        _config_dir: &Path,
    ) -> eyre::Result<AnyChain> {
        let cfg = table_to_solana_config(raw_config)?;
        let chain = SolanaAdapter::new(cfg)?;
        Ok(Arc::new(CachedChain::new(chain)) as AnyChain)
    }

    fn parse_client_id(&self, raw: &str) -> eyre::Result<plugin::AnyClientId> {
        Ok(Box::new(SolanaClientId(raw.to_string())))
    }

    async fn query_status(&self, chain: &AnyChain) -> eyre::Result<ChainStatusInfo> {
        let c = downcast_solana(chain)?;
        let status = c.query_chain_status().await?;
        let height = SolanaCached::chain_status_height(&status);
        let timestamp = SolanaCached::chain_status_timestamp(&status);
        Ok(ChainStatusInfo {
            chain_id: ChainId::from(c.chain_id().to_string()),
            height: height.0,
            timestamp: timestamp.0.to_string(),
        })
    }

    fn chain_id_from_config(&self, _raw: &toml::Table) -> eyre::Result<ChainId> {
        Ok(ChainId::from("n/a"))
    }

    fn rpc_addr_from_config(&self, raw: &toml::Table) -> eyre::Result<String> {
        raw.get("rpc_addr")
            .and_then(toml::Value::as_str)
            .map(String::from)
            .ok_or_else(|| eyre::eyre!("missing 'rpc_addr' in solana config"))
    }

    async fn build_create_client_payload(
        &self,
        _chain: &AnyChain,
    ) -> eyre::Result<Box<dyn Any + Send + Sync>> {
        todo!("build Solana create client payload")
    }

    async fn create_client(
        &self,
        _chain: &AnyChain,
        _payload: Box<dyn Any + Send + Sync>,
    ) -> eyre::Result<String> {
        todo!("create client on Solana chain")
    }
}

pub fn register(registry: &mut ChainRegistry) {
    registry.register_chain(SolanaPlugin);
}
