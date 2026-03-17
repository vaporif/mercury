use std::time::Duration;

use async_trait::async_trait;
use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery};
use mercury_chain_traits::types::{ChainTypes, IbcTypes};
use mercury_core::error::Result;

use crate::CachedChain;

#[async_trait]
impl<C: ChainStatusQuery + IbcTypes> ChainStatusQuery for CachedChain<C>
where
    C::ChainStatus: Clone,
{
    async fn query_chain_status(&self) -> Result<Self::ChainStatus> {
        let ttl = self.inner.block_time() / 2;

        if let Some(status) = self.status.get(ttl) {
            return Ok(status);
        }

        let status = self.inner.query_chain_status().await?;
        self.status.set(status.clone());
        Ok(status)
    }
}

#[async_trait]
impl<X: ChainTypes, C: ClientQuery<X>> ClientQuery<X> for CachedChain<C> {
    async fn query_client_state(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Self::ClientState> {
        let key = format!("{client_id}:{height}");

        if let Some(state) = self.client_states.get(&key) {
            return Ok(state);
        }

        let state = self.inner.query_client_state(client_id, height).await?;
        self.client_states.insert(key, state.clone());
        Ok(state)
    }

    async fn query_consensus_state(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &X::Height,
        query_height: &Self::Height,
    ) -> Result<Self::ConsensusState> {
        let key = format!("{client_id}:{consensus_height}:{query_height}");

        if let Some(state) = self.consensus_states.get(&key) {
            return Ok(state);
        }

        let state = self
            .inner
            .query_consensus_state(client_id, consensus_height, query_height)
            .await?;
        self.consensus_states.insert(key, state.clone());
        Ok(state)
    }

    fn trusting_period(client_state: &Self::ClientState) -> Option<Duration> {
        C::trusting_period(client_state)
    }

    fn client_latest_height(client_state: &Self::ClientState) -> X::Height {
        C::client_latest_height(client_state)
    }
}
