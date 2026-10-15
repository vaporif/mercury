use std::sync::Arc;

use futures::future::BoxFuture;
use mercury_chain_cache::CachedChain;
use mercury_core::plugin::{
    AnyChain, AnyClientId, ClearResult, DynRelay, DynRelayConfig, RelayPairPlugin, SweepScope,
};
use mercury_core::registry::ChainRegistry;
use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
use mercury_cosmos_counterparties::plugin::{downcast_cosmos, dyn_to_worker_config};
use mercury_cosmos_counterparties::wrapper::CosmosAdapter;
use mercury_ethereum_counterparties::wrapper::EthereumAdapter;
use mercury_relay::context::RelayContext;

type CosmosCached = CachedChain<CosmosAdapter<Secp256k1KeyPair>>;
type EthCached = CachedChain<EthereumAdapter>;

fn downcast_eth(chain: &AnyChain) -> eyre::Result<&EthCached> {
    (**chain)
        .downcast_ref::<EthCached>()
        .ok_or_else(|| eyre::eyre!("expected ethereum chain handle"))
}

fn downcast_cosmos_client_id(
    id: &AnyClientId,
) -> eyre::Result<&ibc::core::host::types::identifiers::ClientId> {
    (**id)
        .downcast_ref::<ibc::core::host::types::identifiers::ClientId>()
        .ok_or_else(|| eyre::eyre!("expected cosmos client ID"))
}

fn downcast_evm_client_id(
    id: &AnyClientId,
) -> eyre::Result<&mercury_ethereum_counterparties::types::EvmClientId> {
    (**id)
        .downcast_ref::<mercury_ethereum_counterparties::types::EvmClientId>()
        .ok_or_else(|| eyre::eyre!("expected ethereum client ID"))
}

struct CosmosToEthereumRelay;

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
        src_client_id: &AnyClientId,
        dst_client_id: &AnyClientId,
    ) -> eyre::Result<(Arc<dyn DynRelay>, Arc<dyn DynRelay>)> {
        let src = downcast_cosmos(src)?.clone();
        let dst = downcast_eth(dst)?.clone();
        let src_id = downcast_cosmos_client_id(src_client_id)?.clone();
        let dst_id = downcast_evm_client_id(dst_client_id)?.clone();

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

struct EthereumToCosmosRelay;

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
        src_client_id: &AnyClientId,
        dst_client_id: &AnyClientId,
    ) -> eyre::Result<(Arc<dyn DynRelay>, Arc<dyn DynRelay>)> {
        let src = downcast_eth(src)?.clone();
        let dst = downcast_cosmos(dst)?.clone();
        let src_id = downcast_evm_client_id(src_client_id)?.clone();
        let dst_id = downcast_cosmos_client_id(dst_client_id)?.clone();

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
    fn clear_packets(
        self: Arc<Self>,
        scope: SweepScope,
    ) -> BoxFuture<'static, mercury_core::error::Result<ClearResult>> {
        let inner = Arc::clone(&self.0);
        Box::pin(async move {
            mercury_relay::workers::packet_sweeper::clear_packets_once(inner, scope).await
        })
    }

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
    fn clear_packets(
        self: Arc<Self>,
        scope: SweepScope,
    ) -> BoxFuture<'static, mercury_core::error::Result<ClearResult>> {
        let inner = Arc::clone(&self.0);
        Box::pin(async move {
            mercury_relay::workers::packet_sweeper::clear_packets_once(inner, scope).await
        })
    }

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

pub fn register(registry: &mut ChainRegistry) {
    registry.register_pair(CosmosToEthereumRelay);
    registry.register_pair(EthereumToCosmosRelay);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibc::core::host::types::identifiers::ClientId;
    use mercury_ethereum_counterparties::types::EvmClientId;

    fn not_a_chain() -> AnyChain {
        Arc::new(())
    }

    fn cosmos_client_id() -> AnyClientId {
        Box::new(ClientId::new("08-wasm", 0).unwrap())
    }

    fn evm_client_id() -> AnyClientId {
        Box::new(EvmClientId("client-0".to_string()))
    }

    #[test]
    fn registers_both_directions() {
        let mut registry = ChainRegistry::new();
        register(&mut registry);
        for (src, dst) in [("cosmos", "ethereum"), ("ethereum", "cosmos")] {
            let pair = registry.pair(src, dst).unwrap();
            assert_eq!((pair.src_type(), pair.dst_type()), (src, dst));
        }
        assert!(registry.pair("cosmos", "cosmos").is_err());
        assert!(registry.pair("ethereum", "ethereum").is_err());
    }

    #[test]
    fn cosmos_to_ethereum_checks_source_is_cosmos() {
        let err = CosmosToEthereumRelay
            .build_relay(
                &not_a_chain(),
                &not_a_chain(),
                &cosmos_client_id(),
                &evm_client_id(),
            )
            .err()
            .unwrap();
        assert_eq!(err.to_string(), "expected cosmos chain handle");
    }

    #[test]
    fn ethereum_to_cosmos_checks_source_is_ethereum() {
        let err = EthereumToCosmosRelay
            .build_relay(
                &not_a_chain(),
                &not_a_chain(),
                &evm_client_id(),
                &cosmos_client_id(),
            )
            .err()
            .unwrap();
        assert_eq!(err.to_string(), "expected ethereum chain handle");
    }

    #[test]
    fn eth_handle_downcast_rejects_other_types() {
        assert_eq!(
            downcast_eth(&not_a_chain()).err().unwrap().to_string(),
            "expected ethereum chain handle"
        );
    }

    #[test]
    fn client_id_downcasts_reject_the_other_chains_id() {
        assert_eq!(
            downcast_cosmos_client_id(&cosmos_client_id())
                .unwrap()
                .as_str(),
            "08-wasm-0"
        );
        assert_eq!(
            downcast_evm_client_id(&evm_client_id()).unwrap().0,
            "client-0"
        );
        assert_eq!(
            downcast_cosmos_client_id(&evm_client_id())
                .unwrap_err()
                .to_string(),
            "expected cosmos client ID"
        );
        assert_eq!(
            downcast_evm_client_id(&cosmos_client_id())
                .unwrap_err()
                .to_string(),
            "expected ethereum client ID"
        );
    }
}
