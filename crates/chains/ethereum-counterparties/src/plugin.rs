use std::path::Path;
use std::sync::Arc;

use futures::future::BoxFuture;
use mercury_chain_cache::CachedChain;
use mercury_chain_traits::queries::ChainStatusQuery;
use mercury_chain_traits::types::ChainTypes;
use mercury_core::plugin::{self, AnyChain, AnyClientId, ChainPlugin, ChainStatusInfo};
#[cfg(feature = "cosmos-sp1")]
use mercury_core::plugin::{DynRelay, DynRelayConfig, RelayPairPlugin};
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
    let value = toml::Value::Table(raw.clone());
    value
        .try_into()
        .map_err(|e| eyre::eyre!("invalid ethereum config: {e}"))
}

struct EthereumPlugin;

impl ChainPlugin for EthereumPlugin {
    fn chain_type(&self) -> &'static str {
        "ethereum"
    }

    fn validate_config(&self, raw: &toml::Table) -> eyre::Result<()> {
        let cfg = table_to_eth_config(raw)?;
        cfg.validate()
    }

    fn connect(
        &self,
        raw_config: &toml::Table,
        config_dir: &Path,
    ) -> BoxFuture<'_, eyre::Result<AnyChain>> {
        let raw_config = raw_config.clone();
        let config_dir = config_dir.to_path_buf();
        Box::pin(async move {
            let cfg = table_to_eth_config(&raw_config)?;
            let key_path = config_dir.join(&cfg.key_file);

            #[cfg(unix)]
            plugin::warn_key_file_permissions(&key_path);

            let chain_id_display = cfg.chain_id;
            let signer = load_ethereum_signer(&key_path)
                .map_err(|e| eyre::eyre!("loading signer for chain {chain_id_display}: {e}"))?;

            let chain = EthereumAdapter::new(cfg, signer)
                .await
                .map_err(|e| eyre::eyre!("connecting to chain {chain_id_display}: {e}"))?;

            Ok(Arc::new(CachedChain::new(chain)) as AnyChain)
        })
    }

    fn parse_client_id(&self, raw: &str) -> eyre::Result<AnyClientId> {
        Ok(Box::new(EvmClientId(raw.to_string())))
    }

    fn query_status(&self, chain: &AnyChain) -> BoxFuture<'_, eyre::Result<ChainStatusInfo>> {
        let chain = chain.clone();
        Box::pin(async move {
            let chain = &chain;
            let chain = downcast_eth(chain)?;
            let status = chain.query_chain_status().await?;
            let height = EthCached::chain_status_height(&status);
            let timestamp = EthCached::chain_status_timestamp(&status);
            let chain_id = chain.chain_id();
            Ok(ChainStatusInfo {
                chain_id: chain_id.0.to_string(),
                height: height.0,
                timestamp: timestamp.0.to_string(),
            })
        })
    }

    fn chain_id_from_config(&self, raw: &toml::Table) -> eyre::Result<String> {
        raw.get("chain_id")
            .and_then(toml::Value::as_integer)
            .map(|id| id.to_string())
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
    #[cfg(feature = "cosmos-sp1")]
    {
        registry.register_pair(cross_chain::CosmosToEthereumRelay);
        registry.register_pair(cross_chain::EthereumToCosmosRelay);
    }
}

#[cfg(feature = "cosmos-sp1")]
mod cross_chain {
    use super::{
        AnyChain, Arc, BoxFuture, DynRelay, DynRelayConfig, EthCached, EvmClientId, RelayPairPlugin,
    };
    use mercury_chain_cache::CachedChain;
    use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
    use mercury_cosmos_counterparties::plugin::{downcast_cosmos, dyn_to_worker_config};
    use mercury_cosmos_counterparties::wrapper::CosmosAdapter;
    use mercury_relay::context::RelayContext;

    type CosmosCached = CachedChain<CosmosAdapter<Secp256k1KeyPair>>;

    pub(super) struct CosmosToEthereumRelay;

    impl RelayPairPlugin for CosmosToEthereumRelay {
        fn src_type(&self) -> &'static str {
            "cosmos"
        }

        fn dst_type(&self) -> &'static str {
            "ethereum"
        }

        fn build_relay(
            &self,
            src: &AnyChain,
            dst: &AnyChain,
            src_client_id: &str,
            dst_client_id: &str,
        ) -> eyre::Result<(Arc<dyn DynRelay>, Arc<dyn DynRelay>)> {
            let src = downcast_cosmos(src)?.clone();
            let dst = super::downcast_eth(dst)?.clone();
            let src_id: ibc::core::host::types::identifiers::ClientId = src_client_id
                .parse()
                .map_err(|e| eyre::eyre!("invalid src_client_id: {e}"))?;
            let dst_id = EvmClientId(dst_client_id.to_string());

            let fwd: Arc<dyn DynRelay> = Arc::new(CosmosEthRelay(Arc::new(RelayContext {
                src_chain: src.clone(),
                dst_chain: dst.clone(),
                src_client_id: src_id.clone(),
                dst_client_id: dst_id.clone(),
            })));
            let rev: Arc<dyn DynRelay> = Arc::new(EthCosmosRelay(Arc::new(RelayContext {
                src_chain: dst,
                dst_chain: src,
                src_client_id: dst_id,
                dst_client_id: src_id,
            })));
            Ok((fwd, rev))
        }
    }

    pub(super) struct EthereumToCosmosRelay;

    impl RelayPairPlugin for EthereumToCosmosRelay {
        fn src_type(&self) -> &'static str {
            "ethereum"
        }

        fn dst_type(&self) -> &'static str {
            "cosmos"
        }

        fn build_relay(
            &self,
            src: &AnyChain,
            dst: &AnyChain,
            src_client_id: &str,
            dst_client_id: &str,
        ) -> eyre::Result<(Arc<dyn DynRelay>, Arc<dyn DynRelay>)> {
            let src = super::downcast_eth(src)?.clone();
            let dst = downcast_cosmos(dst)?.clone();
            let src_id = EvmClientId(src_client_id.to_string());
            let dst_id: ibc::core::host::types::identifiers::ClientId = dst_client_id
                .parse()
                .map_err(|e| eyre::eyre!("invalid dst_client_id: {e}"))?;

            let fwd: Arc<dyn DynRelay> = Arc::new(EthCosmosRelay(Arc::new(RelayContext {
                src_chain: src.clone(),
                dst_chain: dst.clone(),
                src_client_id: src_id.clone(),
                dst_client_id: dst_id.clone(),
            })));
            let rev: Arc<dyn DynRelay> = Arc::new(CosmosEthRelay(Arc::new(RelayContext {
                src_chain: dst,
                dst_chain: src,
                src_client_id: dst_id,
                dst_client_id: src_id,
            })));
            Ok((fwd, rev))
        }
    }

    struct CosmosEthRelay(Arc<RelayContext<CosmosCached, EthCached>>);

    impl DynRelay for CosmosEthRelay {
        fn run(
            self: Arc<Self>,
            token: tokio_util::sync::CancellationToken,
            config: DynRelayConfig,
        ) -> BoxFuture<'static, mercury_core::error::Result<()>> {
            let inner = Arc::clone(&self.0);
            Box::pin(async move {
                let worker_config = dyn_to_worker_config(&config)?;
                inner.run_with_token(token, worker_config).await
            })
        }
    }

    struct EthCosmosRelay(Arc<RelayContext<EthCached, CosmosCached>>);

    impl DynRelay for EthCosmosRelay {
        fn run(
            self: Arc<Self>,
            token: tokio_util::sync::CancellationToken,
            config: DynRelayConfig,
        ) -> BoxFuture<'static, mercury_core::error::Result<()>> {
            let inner = Arc::clone(&self.0);
            Box::pin(async move {
                let worker_config = dyn_to_worker_config(&config)?;
                inner.run_with_token(token, worker_config).await
            })
        }
    }
}
