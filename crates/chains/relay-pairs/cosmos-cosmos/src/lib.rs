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
