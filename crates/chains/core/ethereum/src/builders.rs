use alloy::primitives::U256;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use mercury_chain_traits::builders::PacketMessageBuilder;
use mercury_core::error::Result;

use crate::chain::EthereumChain;
use crate::contracts::{ICS26Router, IICS02ClientMsgs, IICS26RouterMsgs};
use crate::types::{EvmAcknowledgement, EvmCommitmentProof, EvmHeight, EvmMessage, EvmPacket};

#[derive(Clone, Debug)]
pub struct CreateClientPayload {
    pub client_state: Vec<u8>,
    pub consensus_state: Vec<u8>,
    pub counterparty_client_id: Option<String>,
    pub counterparty_merkle_prefix: Option<mercury_core::MerklePrefix>,
}

#[derive(Clone, Debug)]
pub struct UpdateClientPayload {
    pub headers: Vec<Vec<u8>>,
    /// Execution block number after applying these headers (for `eth_getProof`).
    pub target_execution_height: Option<EvmHeight>,
    /// Beacon slot after applying these headers. Goes into IBC message
    /// `proof_height` because the WASM contract keys consensus states by slot.
    pub target_slot: Option<u64>,
}

fn evm_packet_to_sol(packet: &EvmPacket) -> IICS26RouterMsgs::Packet {
    IICS26RouterMsgs::Packet {
        sequence: packet.sequence.into(),
        sourceClient: packet.source_client.clone(),
        destClient: packet.dest_client.clone(),
        timeoutTimestamp: packet.timeout_timestamp.into(),
        payloads: packet
            .payloads
            .iter()
            .map(|p| IICS26RouterMsgs::Payload {
                sourcePort: p.source_port.clone().into(),
                destPort: p.dest_port.clone().into(),
                version: p.version.clone(),
                encoding: p.encoding.clone(),
                value: p.value.clone().into(),
            })
            .collect(),
    }
}

#[must_use]
pub fn encode_evm_proof(proof: &EvmCommitmentProof) -> Vec<u8> {
    use alloy::sol_types::SolValue;
    (
        proof.storage_root,
        proof.account_proof.clone(),
        proof.storage_key,
        proof.storage_value,
        proof.storage_proof.clone(),
    )
        .abi_encode()
}

#[async_trait]
impl PacketMessageBuilder<Self> for EthereumChain {
    async fn build_receive_packet_message(
        &self,
        packet: &EvmPacket,
        proof: EvmCommitmentProof,
        proof_height: EvmHeight,
        revision: u64,
    ) -> Result<EvmMessage> {
        let call = ICS26Router::recvPacketCall {
            msg_: IICS26RouterMsgs::MsgRecvPacket {
                packet: evm_packet_to_sol(packet),
                proofCommitment: encode_evm_proof(&proof).into(),
                proofHeight: IICS02ClientMsgs::Height {
                    revisionNumber: revision,
                    revisionHeight: proof_height.0,
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
        proof: EvmCommitmentProof,
        proof_height: EvmHeight,
        revision: u64,
    ) -> Result<EvmMessage> {
        let call = ICS26Router::ackPacketCall {
            msg_: IICS26RouterMsgs::MsgAckPacket {
                packet: evm_packet_to_sol(packet),
                acknowledgement: ack.0.clone().into(),
                proofAcked: encode_evm_proof(&proof).into(),
                proofHeight: IICS02ClientMsgs::Height {
                    revisionNumber: revision,
                    revisionHeight: proof_height.0,
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
        proof: EvmCommitmentProof,
        proof_height: EvmHeight,
        revision: u64,
    ) -> Result<EvmMessage> {
        let call = ICS26Router::timeoutPacketCall {
            msg_: IICS26RouterMsgs::MsgTimeoutPacket {
                packet: evm_packet_to_sol(packet),
                proofTimeout: encode_evm_proof(&proof).into(),
                proofHeight: IICS02ClientMsgs::Height {
                    revisionNumber: revision,
                    revisionHeight: proof_height.0,
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
