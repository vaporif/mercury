use std::time::Duration;

use async_trait::async_trait;
use mercury_core::error::Result;

use crate::types::{HasChainTypes, HasIbcTypes};

/// Queries the current status (height and timestamp) of the chain.
#[async_trait]
pub trait CanQueryChainStatus: HasChainTypes {
    async fn query_chain_status(&self) -> Result<Self::ChainStatus>;
}

/// Queries and inspects IBC client and consensus state.
#[async_trait]
pub trait CanQueryClient<Counterparty: HasChainTypes + ?Sized>: HasIbcTypes<Counterparty> {
    async fn query_client_state(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Self::ClientState>;

    async fn query_consensus_state(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &Counterparty::Height,
        query_height: &Self::Height,
    ) -> Result<Self::ConsensusState>;

    fn trusting_period(client_state: &Self::ClientState) -> Option<Duration>;

    fn client_latest_height(client_state: &Self::ClientState) -> Counterparty::Height;
}

/// Queries packet commitments, receipts, and acknowledgements at a given height.
#[async_trait]
pub trait CanQueryPacketState<Counterparty: HasChainTypes + ?Sized>:
    HasIbcTypes<Counterparty>
{
    async fn query_packet_commitment(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::PacketCommitment>, Self::CommitmentProof)>;

    async fn query_packet_receipt(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::PacketReceipt>, Self::CommitmentProof)>;

    async fn query_packet_acknowledgement(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::Acknowledgement>, Self::CommitmentProof)>;
}
