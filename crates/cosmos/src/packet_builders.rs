use async_trait::async_trait;

use mercury_chain_traits::packet_builders::{
    CanBuildAckPacketMessage, CanBuildReceivePacketMessage, CanBuildTimeoutPacketMessage,
};
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::types::{CosmosMessage, CosmosPacket, MerkleProof, PacketAcknowledgement};

#[derive(Clone, Debug)]
pub struct CosmosReceivePacketPayload {
    pub proof: MerkleProof,
    pub proof_height: tendermint::block::Height,
}

#[derive(Clone, Debug)]
pub struct CosmosAckPacketPayload {
    pub proof: MerkleProof,
    pub proof_height: tendermint::block::Height,
}

#[derive(Clone, Debug)]
pub struct CosmosTimeoutPacketPayload {
    pub proof: MerkleProof,
    pub proof_height: tendermint::block::Height,
}

#[async_trait]
impl CanBuildReceivePacketMessage<Self> for CosmosChain {
    type ReceivePacketPayload = CosmosReceivePacketPayload;

    async fn build_receive_packet_message(
        &self,
        _packet: &CosmosPacket,
        _payload: Self::ReceivePacketPayload,
    ) -> Result<CosmosMessage> {
        // TODO: encode MsgRecvPacket proto message
        todo!("build receive packet message")
    }
}

#[async_trait]
impl CanBuildAckPacketMessage<Self> for CosmosChain {
    type AckPacketPayload = CosmosAckPacketPayload;

    async fn build_ack_packet_message(
        &self,
        _packet: &CosmosPacket,
        _ack: &PacketAcknowledgement,
        _payload: Self::AckPacketPayload,
    ) -> Result<CosmosMessage> {
        // TODO: encode MsgAcknowledgement proto message
        todo!("build ack packet message")
    }
}

#[async_trait]
impl CanBuildTimeoutPacketMessage<Self> for CosmosChain {
    type TimeoutPacketPayload = CosmosTimeoutPacketPayload;

    async fn build_timeout_packet_message(
        &self,
        _packet: &CosmosPacket,
        _payload: Self::TimeoutPacketPayload,
    ) -> Result<CosmosMessage> {
        // TODO: encode MsgTimeout proto message
        todo!("build timeout packet message")
    }
}
