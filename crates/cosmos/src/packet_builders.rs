use async_trait::async_trait;

use ibc_proto::ibc::core::channel::v1::{
    MsgAcknowledgement, MsgRecvPacket, MsgTimeout, Packet as ProtoPacket,
};
use ibc_proto::ibc::core::client::v1::Height as ProtoHeight;
use mercury_chain_traits::packet_builders::{
    CanBuildAckPacketMessage, CanBuildReceivePacketMessage, CanBuildTimeoutPacketMessage,
};
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::encoding::to_any;
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

fn cosmos_packet_to_proto(packet: &CosmosPacket) -> ProtoPacket {
    let (source_port, destination_port, data) = packet
        .payloads
        .first()
        .map(|p| (p.source_port.clone(), p.dest_port.clone(), p.data.clone()))
        .unwrap_or_default();

    ProtoPacket {
        sequence: packet.sequence,
        source_port,
        source_channel: packet.source_client_id.to_string(),
        destination_port,
        destination_channel: packet.dest_client_id.to_string(),
        data,
        timeout_height: None,
        timeout_timestamp: packet.timeout_timestamp,
    }
}

fn to_proto_height(h: tendermint::block::Height) -> ProtoHeight {
    ProtoHeight {
        revision_number: 0,
        revision_height: h.value(),
    }
}

#[async_trait]
impl CanBuildReceivePacketMessage<Self> for CosmosChain {
    type ReceivePacketPayload = CosmosReceivePacketPayload;

    async fn build_receive_packet_message(
        &self,
        packet: &CosmosPacket,
        payload: Self::ReceivePacketPayload,
    ) -> Result<CosmosMessage> {
        let msg = MsgRecvPacket {
            packet: Some(cosmos_packet_to_proto(packet)),
            proof_commitment: payload.proof.proof_bytes,
            proof_height: Some(to_proto_height(payload.proof_height)),
            signer: self.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }
}

#[async_trait]
impl CanBuildAckPacketMessage<Self> for CosmosChain {
    type AckPacketPayload = CosmosAckPacketPayload;

    async fn build_ack_packet_message(
        &self,
        packet: &CosmosPacket,
        ack: &PacketAcknowledgement,
        payload: Self::AckPacketPayload,
    ) -> Result<CosmosMessage> {
        let msg = MsgAcknowledgement {
            packet: Some(cosmos_packet_to_proto(packet)),
            acknowledgement: ack.0.clone(),
            proof_acked: payload.proof.proof_bytes,
            proof_height: Some(to_proto_height(payload.proof_height)),
            signer: self.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }
}

#[async_trait]
impl CanBuildTimeoutPacketMessage<Self> for CosmosChain {
    type TimeoutPacketPayload = CosmosTimeoutPacketPayload;

    async fn build_timeout_packet_message(
        &self,
        packet: &CosmosPacket,
        payload: Self::TimeoutPacketPayload,
    ) -> Result<CosmosMessage> {
        let msg = MsgTimeout {
            packet: Some(cosmos_packet_to_proto(packet)),
            proof_unreceived: payload.proof.proof_bytes,
            proof_height: Some(to_proto_height(payload.proof_height)),
            next_sequence_recv: packet.sequence,
            signer: self.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }
}
