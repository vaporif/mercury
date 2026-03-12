use std::time::Duration;

use async_trait::async_trait;
use mercury_core::error::Result;

use crate::types::{HasChainStatusType, HasChainTypes, HasIbcTypes};

/// Queries the current status (height and timestamp) of the chain.
#[async_trait]
pub trait CanQueryChainStatus: HasChainStatusType {
    async fn query_chain_status(&self) -> Result<Self::ChainStatus>;
}

/// Queries the client state for a given client ID at a specific height.
#[async_trait]
pub trait CanQueryClientState<Counterparty: HasChainTypes + ?Sized>:
    HasIbcTypes<Counterparty>
{
    async fn query_client_state(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Self::ClientState>;
}

/// Queries the consensus state for a given client at a specific consensus height.
#[async_trait]
pub trait CanQueryConsensusState<Counterparty: HasChainTypes + ?Sized>:
    HasIbcTypes<Counterparty>
{
    async fn query_consensus_state(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &Counterparty::Height,
        query_height: &Self::Height,
    ) -> Result<Self::ConsensusState>;
}

/// Provides the trusting period from a client state.
pub trait HasTrustingPeriod<Counterparty: HasChainTypes + ?Sized>:
    HasIbcTypes<Counterparty>
{
    fn trusting_period(client_state: &Self::ClientState) -> Option<Duration>;
}

/// Extracts the latest height tracked by a client state.
pub trait HasClientLatestHeight<Counterparty: HasChainTypes + ?Sized>:
    HasIbcTypes<Counterparty>
{
    fn client_latest_height(client_state: &Self::ClientState) -> Counterparty::Height;
}
