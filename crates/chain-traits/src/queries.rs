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
