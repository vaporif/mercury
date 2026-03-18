mod bounded_cache;
mod coordinator;
mod passthrough;
mod queries;

use std::sync::Arc;

use mercury_chain_traits::types::{IbcTypes, MessageSender};

use bounded_cache::{BoundedCache, TtlCell};
use coordinator::{TxCoordinatorHandle, spawn_coordinator};

const CLIENT_CACHE_CAP: usize = 20;

#[derive(Clone)]
pub struct CachedChain<C: IbcTypes> {
    inner: C,
    status: Arc<TtlCell<C::ChainStatus>>,
    client_states: Arc<BoundedCache<C::ClientState>>,
    consensus_states: Arc<BoundedCache<C::ConsensusState>>,
    tx_handle: TxCoordinatorHandle<C::Message>,
}

impl<C: IbcTypes> CachedChain<C> {
    pub const fn inner(&self) -> &C {
        &self.inner
    }
}

impl<C> CachedChain<C>
where
    C: IbcTypes + MessageSender + Clone + Send + 'static,
    C::Message: Send + 'static,
{
    pub fn new(inner: C) -> Self {
        let tx_handle = spawn_coordinator(inner.clone());
        Self {
            inner,
            status: Arc::new(TtlCell::new()),
            client_states: Arc::new(BoundedCache::new(CLIENT_CACHE_CAP)),
            consensus_states: Arc::new(BoundedCache::new(CLIENT_CACHE_CAP)),
            tx_handle,
        }
    }
}
