use async_trait::async_trait;

use mercury_chain_traits::packet_queries::{
    CanQueryPacketAcknowledgement, CanQueryPacketCommitment, CanQueryPacketReceipt,
};
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::types::{PacketCommitment, MerkleProof, PacketReceipt, PacketAcknowledgement};

#[async_trait]
impl CanQueryPacketCommitment<Self> for CosmosChain {
    async fn query_packet_commitment(
        &self,
        _client_id: &Self::ClientId,
        _sequence: u64,
        _height: &Self::Height,
    ) -> Result<(Option<PacketCommitment>, MerkleProof)> {
        // TODO: ABCI query at path "{client_id}\x01{sequence_be_bytes}" with prove=true
        todo!("query packet commitment")
    }
}

#[async_trait]
impl CanQueryPacketReceipt<Self> for CosmosChain {
    async fn query_packet_receipt(
        &self,
        _client_id: &Self::ClientId,
        _sequence: u64,
        _height: &Self::Height,
    ) -> Result<(Option<PacketReceipt>, MerkleProof)> {
        // TODO: ABCI query at path "{client_id}\x02{sequence_be_bytes}" with prove=true
        todo!("query packet receipt")
    }
}

#[async_trait]
impl CanQueryPacketAcknowledgement<Self> for CosmosChain {
    async fn query_packet_acknowledgement(
        &self,
        _client_id: &Self::ClientId,
        _sequence: u64,
        _height: &Self::Height,
    ) -> Result<(Option<PacketAcknowledgement>, MerkleProof)> {
        // TODO: ABCI query at path "{client_id}\x03{sequence_be_bytes}" with prove=true
        todo!("query packet ack")
    }
}
