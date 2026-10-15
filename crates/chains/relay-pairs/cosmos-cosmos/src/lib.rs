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
use mercury_relay::context::RelayContext;

type CosmosCached = CachedChain<CosmosAdapter<Secp256k1KeyPair>>;

fn downcast_cosmos_client_id(
    id: &AnyClientId,
) -> eyre::Result<&ibc::core::host::types::identifiers::ClientId> {
    (**id)
        .downcast_ref::<ibc::core::host::types::identifiers::ClientId>()
        .ok_or_else(|| eyre::eyre!("expected cosmos client ID"))
}

struct CosmosRelayContext(Arc<RelayContext<CosmosCached, CosmosCached>>);

impl DynRelay for CosmosRelayContext {
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

struct CosmosToCosmosRelay;

impl RelayPairPlugin for CosmosToCosmosRelay {
    fn src_type(&self) -> &'static str {
        "cosmos"
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
        let src = downcast_cosmos(src)?.clone();
        let dst = downcast_cosmos(dst)?.clone();
        let src_id = downcast_cosmos_client_id(src_client_id)?.clone();
        let dst_id = downcast_cosmos_client_id(dst_client_id)?.clone();

        let fwd: Arc<dyn DynRelay> = Arc::new(CosmosRelayContext(Arc::new(RelayContext {
            src_chain: src.clone(),
            dst_chain: dst.clone(),
            src_client_id: src_id.clone(),
            dst_client_id: dst_id.clone(),
        })));
        let rev: Arc<dyn DynRelay> = Arc::new(CosmosRelayContext(Arc::new(RelayContext {
            src_chain: dst,
            dst_chain: src,
            src_client_id: dst_id,
            dst_client_id: src_id,
        })));
        Ok((fwd, rev))
    }
}

pub fn register(registry: &mut ChainRegistry) {
    registry.register_pair(CosmosToCosmosRelay);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibc::core::host::types::identifiers::ClientId;

    fn not_a_chain() -> AnyChain {
        Arc::new(())
    }

    fn cosmos_client_id() -> AnyClientId {
        Box::new(ClientId::new("07-tendermint", 0).unwrap())
    }

    #[test]
    fn registers_only_the_cosmos_to_cosmos_pair() {
        let mut registry = ChainRegistry::new();
        register(&mut registry);
        let pair = registry.pair("cosmos", "cosmos").unwrap();
        assert_eq!((pair.src_type(), pair.dst_type()), ("cosmos", "cosmos"));
        assert!(registry.pair("cosmos", "ethereum").is_err());
        assert!(registry.chain("cosmos").is_err());
    }

    #[test]
    fn build_relay_rejects_non_cosmos_chain_handles() {
        let err = CosmosToCosmosRelay
            .build_relay(
                &not_a_chain(),
                &not_a_chain(),
                &cosmos_client_id(),
                &cosmos_client_id(),
            )
            .err()
            .unwrap();
        assert_eq!(err.to_string(), "expected cosmos chain handle");
    }

    #[test]
    fn cosmos_client_id_downcast_accepts_ibc_client_id_only() {
        let id = cosmos_client_id();
        assert_eq!(
            downcast_cosmos_client_id(&id).unwrap().as_str(),
            "07-tendermint-0"
        );
        let wrong: AnyClientId = Box::new("07-tendermint-0".to_string());
        assert_eq!(
            downcast_cosmos_client_id(&wrong).unwrap_err().to_string(),
            "expected cosmos client ID"
        );
    }
}
