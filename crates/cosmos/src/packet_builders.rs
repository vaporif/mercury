use async_trait::async_trait;

use ibc_proto::ibc::core::client::v1::Height as ProtoHeight;
use mercury_chain_traits::packet_builders::{
    CanBuildAckPacketMessage, CanBuildReceivePacketMessage, CanBuildTimeoutPacketMessage,
};
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::encoding::to_any;
use crate::ibc_v2::channel::{
    self, MsgAcknowledgement, MsgRecvPacket, MsgTimeout, Packet as V2Packet,
};
use crate::keys::CosmosSigner;
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

fn cosmos_packet_to_v2(packet: &CosmosPacket) -> V2Packet {
    V2Packet {
        sequence: packet.sequence,
        source_client: packet.source_client_id.to_string(),
        destination_client: packet.dest_client_id.to_string(),
        timeout_timestamp: packet.timeout_timestamp,
        payloads: packet
            .payloads
            .iter()
            .map(|p| channel::Payload {
                source_port: p.source_port.clone(),
                destination_port: p.dest_port.clone(),
                version: p.version.clone(),
                encoding: p.encoding.clone(),
                value: p.data.clone(), // proto `value` = our `data`
            })
            .collect(),
    }
}

fn to_proto_height(revision_number: u64, h: tendermint::block::Height) -> ProtoHeight {
    ProtoHeight {
        revision_number,
        revision_height: h.value(),
    }
}

#[async_trait]
impl<S: CosmosSigner> CanBuildReceivePacketMessage<Self> for CosmosChain<S> {
    type ReceivePacketPayload = CosmosReceivePacketPayload;

    async fn build_receive_packet_message(
        &self,
        packet: &CosmosPacket,
        payload: Self::ReceivePacketPayload,
    ) -> Result<CosmosMessage> {
        let msg = MsgRecvPacket {
            packet: Some(cosmos_packet_to_v2(packet)),
            proof_commitment: payload.proof.proof_bytes,
            proof_height: Some(to_proto_height(
                self.chain_id.revision_number(),
                payload.proof_height,
            )),
            signer: self.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }
}

#[async_trait]
impl<S: CosmosSigner> CanBuildAckPacketMessage<Self> for CosmosChain<S> {
    type AckPacketPayload = CosmosAckPacketPayload;

    async fn build_ack_packet_message(
        &self,
        packet: &CosmosPacket,
        ack: &PacketAcknowledgement,
        payload: Self::AckPacketPayload,
    ) -> Result<CosmosMessage> {
        let msg = MsgAcknowledgement {
            packet: Some(cosmos_packet_to_v2(packet)),
            acknowledgement: Some(channel::Acknowledgement {
                app_acknowledgements: vec![ack.0.clone()],
            }),
            proof_acked: payload.proof.proof_bytes,
            proof_height: Some(to_proto_height(
                self.chain_id.revision_number(),
                payload.proof_height,
            )),
            signer: self.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }
}

#[async_trait]
impl<S: CosmosSigner> CanBuildTimeoutPacketMessage<Self> for CosmosChain<S> {
    type TimeoutPacketPayload = CosmosTimeoutPacketPayload;

    async fn build_timeout_packet_message(
        &self,
        packet: &CosmosPacket,
        payload: Self::TimeoutPacketPayload,
    ) -> Result<CosmosMessage> {
        let msg = MsgTimeout {
            packet: Some(cosmos_packet_to_v2(packet)),
            proof_unreceived: payload.proof.proof_bytes,
            proof_height: Some(to_proto_height(
                self.chain_id.revision_number(),
                payload.proof_height,
            )),
            signer: self.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }
}
