mod bounded_cache;
mod passthrough;
mod queries;

use std::sync::Arc;

use mercury_chain_traits::types::IbcTypes;

use bounded_cache::{BoundedCache, TtlCell};

const CLIENT_CACHE_CAP: usize = 20;

#[derive(Clone)]
pub struct CachedChain<C: IbcTypes> {
    inner: C,
    status: Arc<TtlCell<C::ChainStatus>>,
    client_states: Arc<BoundedCache<C::ClientState>>,
    consensus_states: Arc<BoundedCache<C::ConsensusState>>,
}

impl<C: IbcTypes> CachedChain<C> {
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            status: Arc::new(TtlCell::new()),
            client_states: Arc::new(BoundedCache::new(CLIENT_CACHE_CAP)),
            consensus_states: Arc::new(BoundedCache::new(CLIENT_CACHE_CAP)),
        }
    }
}
