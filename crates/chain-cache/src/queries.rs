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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testhelpers::Probe;

    fn count(probe: &Probe, call: &str) -> usize {
        probe.calls().iter().filter(|c| **c == call).count()
    }

    #[tokio::test]
    async fn chain_status_is_cached_within_ttl() {
        let probe = Probe::new();
        let chain = CachedChain::new(probe.clone());

        let first = chain.query_chain_status().await.unwrap();
        let second = chain.query_chain_status().await.unwrap();

        assert_eq!(first, second);
        assert_eq!(count(&probe, "query_chain_status"), 1);
    }

    #[tokio::test]
    async fn client_state_is_cached_per_client_and_height() {
        let probe = Probe::new();
        let chain = CachedChain::new(probe.clone());
        let client = "client-0".to_owned();

        assert_eq!(chain.query_client_state(&client, &5).await.unwrap(), 6);
        assert_eq!(chain.query_client_state(&client, &5).await.unwrap(), 6);
        assert_eq!(chain.query_client_state(&client, &6).await.unwrap(), 7);
        assert_eq!(
            chain
                .query_client_state(&"client-1".to_owned(), &5)
                .await
                .unwrap(),
            6
        );

        assert_eq!(count(&probe, "query_client_state"), 3);
    }

    #[tokio::test]
    async fn consensus_state_is_cached_per_key() {
        let probe = Probe::new();
        let chain = CachedChain::new(probe.clone());
        let client = "client-0".to_owned();

        assert_eq!(
            chain.query_consensus_state(&client, &2, &3).await.unwrap(),
            5
        );
        assert_eq!(
            chain.query_consensus_state(&client, &2, &3).await.unwrap(),
            5
        );
        assert_eq!(
            chain.query_consensus_state(&client, &3, &2).await.unwrap(),
            5
        );

        assert_eq!(count(&probe, "query_consensus_state"), 2);
    }

    #[test]
    fn client_state_helpers_delegate() {
        assert_eq!(
            <CachedChain<Probe> as ClientQuery<Probe>>::trusting_period(&30),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            <CachedChain<Probe> as ClientQuery<Probe>>::client_latest_height(&30),
            60
        );
    }
}
