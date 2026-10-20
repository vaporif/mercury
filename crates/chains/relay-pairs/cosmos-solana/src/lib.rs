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
use mercury_solana_counterparties::plugin::downcast_solana;
use mercury_solana_counterparties::types::SolanaClientId;
use mercury_solana_counterparties::wrapper::SolanaAdapter;

type CosmosCached = CachedChain<CosmosAdapter<Secp256k1KeyPair>>;
type SolanaCached = CachedChain<SolanaAdapter>;

fn downcast_cosmos_client_id(
    id: &AnyClientId,
) -> eyre::Result<&ibc::core::host::types::identifiers::ClientId> {
    (**id)
        .downcast_ref::<ibc::core::host::types::identifiers::ClientId>()
        .ok_or_else(|| eyre::eyre!("expected cosmos client ID"))
}

fn downcast_solana_client_id(id: &AnyClientId) -> eyre::Result<&SolanaClientId> {
    (**id)
        .downcast_ref::<SolanaClientId>()
        .ok_or_else(|| eyre::eyre!("expected solana client ID"))
}

struct CosmosToSolanaRelay;

impl RelayPairPlugin for CosmosToSolanaRelay {
    fn src_type(&self) -> &'static str {
        "cosmos"
    }

    fn dst_type(&self) -> &'static str {
        "solana"
    }

    fn build_relay(
        &self,
        src: &AnyChain,
        dst: &AnyChain,
        src_client_id: &AnyClientId,
        dst_client_id: &AnyClientId,
    ) -> eyre::Result<(Arc<dyn DynRelay>, Arc<dyn DynRelay>)> {
        let src = downcast_cosmos(src)?.clone();
        let dst = downcast_solana(dst)?.clone();
        let src_id = downcast_cosmos_client_id(src_client_id)?.clone();
        let dst_id = downcast_solana_client_id(dst_client_id)?.clone();

        let fwd: Arc<dyn DynRelay> = Arc::new(CosmosSolRelay(Arc::new(RelayContext {
            src_chain: src.clone(),
            dst_chain: dst.clone(),
            src_client_id: src_id.clone(),
            dst_client_id: dst_id.clone(),
        })));
        let rev: Arc<dyn DynRelay> = Arc::new(SolCosmosRelay(Arc::new(RelayContext {
            src_chain: dst,
            dst_chain: src,
            src_client_id: dst_id,
            dst_client_id: src_id,
        })));
        Ok((fwd, rev))
    }
}

struct SolanaToCosmosRelay;

impl RelayPairPlugin for SolanaToCosmosRelay {
    fn src_type(&self) -> &'static str {
        "solana"
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
        let src = downcast_solana(src)?.clone();
        let dst = downcast_cosmos(dst)?.clone();
        let src_id = downcast_solana_client_id(src_client_id)?.clone();
        let dst_id = downcast_cosmos_client_id(dst_client_id)?.clone();

        let fwd: Arc<dyn DynRelay> = Arc::new(SolCosmosRelay(Arc::new(RelayContext {
            src_chain: src.clone(),
            dst_chain: dst.clone(),
            src_client_id: src_id.clone(),
            dst_client_id: dst_id.clone(),
        })));
        let rev: Arc<dyn DynRelay> = Arc::new(CosmosSolRelay(Arc::new(RelayContext {
            src_chain: dst,
            dst_chain: src,
            src_client_id: dst_id,
            dst_client_id: src_id,
        })));
        Ok((fwd, rev))
    }
}

struct CosmosSolRelay(Arc<RelayContext<CosmosCached, SolanaCached>>);

impl DynRelay for CosmosSolRelay {
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

struct SolCosmosRelay(Arc<RelayContext<SolanaCached, CosmosCached>>);

impl DynRelay for SolCosmosRelay {
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
    registry.register_pair(CosmosToSolanaRelay);
    registry.register_pair(SolanaToCosmosRelay);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibc::core::host::types::identifiers::ClientId;

    fn not_a_chain() -> AnyChain {
        Arc::new(())
    }

    fn cosmos_client_id() -> AnyClientId {
        Box::new(ClientId::new("08-wasm", 1).unwrap())
    }

    fn solana_client_id() -> AnyClientId {
        Box::new(SolanaClientId("07-tendermint-2".to_string()))
    }

    #[test]
    fn registers_both_directions() {
        let mut registry = ChainRegistry::new();
        register(&mut registry);
        for (src, dst) in [("cosmos", "solana"), ("solana", "cosmos")] {
            let pair = registry.pair(src, dst).unwrap();
            assert_eq!((pair.src_type(), pair.dst_type()), (src, dst));
        }
        assert!(registry.pair("solana", "solana").is_err());
        assert!(registry.pair("cosmos", "cosmos").is_err());
    }

    #[test]
    fn cosmos_to_solana_checks_source_is_cosmos() {
        let err = CosmosToSolanaRelay
            .build_relay(
                &not_a_chain(),
                &not_a_chain(),
                &cosmos_client_id(),
                &solana_client_id(),
            )
            .err()
            .unwrap();
        assert_eq!(err.to_string(), "expected cosmos chain handle");
    }

    #[test]
    fn solana_to_cosmos_checks_source_is_solana() {
        let err = SolanaToCosmosRelay
            .build_relay(
                &not_a_chain(),
                &not_a_chain(),
                &solana_client_id(),
                &cosmos_client_id(),
            )
            .err()
            .unwrap();
        assert_eq!(err.to_string(), "expected solana chain handle");
    }

    #[test]
    fn client_id_downcasts_reject_the_other_chains_id() {
        assert_eq!(
            downcast_cosmos_client_id(&cosmos_client_id())
                .unwrap()
                .as_str(),
            "08-wasm-1"
        );
        assert_eq!(
            downcast_solana_client_id(&solana_client_id()).unwrap().0,
            "07-tendermint-2"
        );
        assert_eq!(
            downcast_cosmos_client_id(&solana_client_id())
                .unwrap_err()
                .to_string(),
            "expected cosmos client ID"
        );
        assert_eq!(
            downcast_solana_client_id(&cosmos_client_id())
                .unwrap_err()
                .to_string(),
            "expected solana client ID"
        );
    }
}
