use async_trait::async_trait;
use mercury_core::error::Result;

use crate::types::{HasChainTypes, HasPacketTypes};

#[async_trait]
pub trait CanQueryPacketCommitment<Counterparty: HasChainTypes + ?Sized>:
    HasPacketTypes<Counterparty>
{
    async fn query_packet_commitment(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::PacketCommitment>, Self::CommitmentProof)>;
}

#[async_trait]
pub trait CanQueryPacketReceipt<Counterparty: HasChainTypes + ?Sized>:
    HasPacketTypes<Counterparty>
{
    async fn query_packet_receipt(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::PacketReceipt>, Self::CommitmentProof)>;
}

#[async_trait]
pub trait CanQueryPacketAcknowledgement<Counterparty: HasChainTypes + ?Sized>:
    HasPacketTypes<Counterparty>
{
    async fn query_packet_acknowledgement(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::Acknowledgement>, Self::CommitmentProof)>;
}
