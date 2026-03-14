use alloy::primitives::U256;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use mercury_chain_traits::builders::PacketMessageBuilder;
use mercury_core::error::Result;

use crate::chain::EthereumChain;
use crate::contracts::{ICS26Router, IICS02ClientMsgs, IICS26RouterMsgs};
use crate::types::{EvmAcknowledgement, EvmHeight, EvmMessage, EvmPacket};

#[derive(Clone, Debug)]
pub struct CreateClientPayload {
    pub client_state: Vec<u8>,
    pub consensus_state: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct UpdateClientPayload {
    pub headers: Vec<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct ReceivePacketPayload {
    pub proof_commitment: Vec<u8>,
    pub proof_height: EvmHeight,
}

#[derive(Clone, Debug)]
pub struct AckPacketPayload {
    pub proof_acked: Vec<u8>,
    pub proof_height: EvmHeight,
}

#[derive(Clone, Debug)]
pub struct TimeoutPacketPayload {
    pub proof_timeout: Vec<u8>,
    pub proof_height: EvmHeight,
}

fn evm_packet_to_sol(packet: &EvmPacket) -> IICS26RouterMsgs::Packet {
    IICS26RouterMsgs::Packet {
        sequence: packet.sequence,
        sourceClient: packet.source_client.clone(),
        destClient: packet.dest_client.clone(),
        timeoutTimestamp: packet.timeout_timestamp,
        payloads: packet
            .payloads
            .iter()
            .map(|p| IICS26RouterMsgs::Payload {
                sourcePort: p.source_port.clone(),
                destPort: p.dest_port.clone(),
                version: p.version.clone(),
                encoding: p.encoding.clone(),
                value: p.value.clone().into(),
            })
            .collect(),
    }
}

#[async_trait]
impl PacketMessageBuilder<Self> for EthereumChain {
    type ReceivePacketPayload = ReceivePacketPayload;
    type AckPacketPayload = AckPacketPayload;
    type TimeoutPacketPayload = TimeoutPacketPayload;

    async fn build_receive_packet_message(
        &self,
        packet: &EvmPacket,
        payload: ReceivePacketPayload,
    ) -> Result<EvmMessage> {
        let call = ICS26Router::recvPacketCall {
            msg_: IICS26RouterMsgs::MsgRecvPacket {
                packet: evm_packet_to_sol(packet),
                proofCommitment: payload.proof_commitment.into(),
                proofHeight: IICS02ClientMsgs::Height {
                    revisionNumber: 0,
                    revisionHeight: payload.proof_height.0,
                },
            },
        };

        Ok(EvmMessage {
            to: self.router_address,
            calldata: call.abi_encode(),
            value: U256::ZERO,
        })
    }

    async fn build_ack_packet_message(
        &self,
        packet: &EvmPacket,
        ack: &EvmAcknowledgement,
        payload: AckPacketPayload,
    ) -> Result<EvmMessage> {
        let call = ICS26Router::ackPacketCall {
            msg_: IICS26RouterMsgs::MsgAckPacket {
                packet: evm_packet_to_sol(packet),
                acknowledgement: ack.0.clone().into(),
                proofAcked: payload.proof_acked.into(),
                proofHeight: IICS02ClientMsgs::Height {
                    revisionNumber: 0,
                    revisionHeight: payload.proof_height.0,
                },
            },
        };

        Ok(EvmMessage {
            to: self.router_address,
            calldata: call.abi_encode(),
            value: U256::ZERO,
        })
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &EvmPacket,
        payload: TimeoutPacketPayload,
    ) -> Result<EvmMessage> {
        let call = ICS26Router::timeoutPacketCall {
            msg_: IICS26RouterMsgs::MsgTimeoutPacket {
                packet: evm_packet_to_sol(packet),
                proofTimeout: payload.proof_timeout.into(),
                proofHeight: IICS02ClientMsgs::Height {
                    revisionNumber: 0,
                    revisionHeight: payload.proof_height.0,
                },
            },
        };

        Ok(EvmMessage {
            to: self.router_address,
            calldata: call.abi_encode(),
            value: U256::ZERO,
        })
    }
}
