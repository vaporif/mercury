use std::time::Duration;

use async_trait::async_trait;
use mercury_core::error::Result;

use crate::types::{HasChainStatusType, HasChainTypes, HasIbcTypes};

#[async_trait]
pub trait CanQueryChainStatus: HasChainStatusType {
    async fn query_chain_status(&self) -> Result<Self::ChainStatus>;
}

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

pub trait HasTrustingPeriod<Counterparty: HasChainTypes + ?Sized>:
    HasIbcTypes<Counterparty>
{
    fn trusting_period(client_state: &Self::ClientState) -> Option<Duration>;
}

pub trait HasClientLatestHeight<Counterparty: HasChainTypes + ?Sized>:
    HasIbcTypes<Counterparty>
{
    fn client_latest_height(client_state: &Self::ClientState) -> Counterparty::Height;
}
