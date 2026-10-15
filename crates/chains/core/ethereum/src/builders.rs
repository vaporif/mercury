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
    /// Execution block number after these headers land (for `eth_getProof`).
    pub target_execution_height: Option<EvmHeight>,
    /// Beacon slot after these headers. Used as IBC `proof_height` since
    /// the WASM contract keys consensus states by slot.
    pub target_slot: Option<u64>,
    /// Earliest dst-chain timestamp (secs) when this update can land.
    pub required_dst_timestamp_secs: Option<u64>,
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

#[cfg(test)]
mod tests {
    use alloy::primitives::{B256, Bytes};
    use alloy::sol_types::SolValue;
    use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};

    use super::*;
    use crate::chain::test_support::{ROUTER, offline_chain};
    use crate::types::{EvmPayload, ProofHeight};

    type ProofTuple = (B256, Vec<Bytes>, B256, U256, Vec<Bytes>);

    fn proof() -> EvmCommitmentProof {
        EvmCommitmentProof {
            proof_height: ProofHeight(10),
            storage_root: B256::repeat_byte(1),
            account_proof: vec![vec![2, 2], vec![3]],
            storage_key: B256::repeat_byte(4),
            storage_value: U256::from(5),
            storage_proof: vec![vec![6]],
        }
    }

    fn packet() -> EvmPacket {
        EvmPacket {
            source_client: "src-0".to_string(),
            dest_client: "dst-0".to_string(),
            sequence: PacketSequence(12),
            timeout_timestamp: TimeoutTimestamp(1_800_000_000),
            payloads: vec![EvmPayload {
                source_port: Port("transfer".to_string()),
                dest_port: Port("transfer".to_string()),
                version: "ics20-1".to_string(),
                encoding: "application/x-solidity-abi".to_string(),
                value: vec![0xde, 0xad],
            }],
        }
    }

    fn assert_packet(sol: &IICS26RouterMsgs::Packet) {
        assert_eq!(sol.sequence, 12);
        assert_eq!(sol.sourceClient, "src-0");
        assert_eq!(sol.destClient, "dst-0");
        assert_eq!(sol.timeoutTimestamp, 1_800_000_000);
        assert_eq!(sol.payloads.len(), 1);
        assert_eq!(sol.payloads[0].sourcePort, "transfer");
        assert_eq!(sol.payloads[0].version, "ics20-1");
        assert_eq!(sol.payloads[0].encoding, "application/x-solidity-abi");
        assert_eq!(sol.payloads[0].value.as_ref(), &[0xde, 0xad]);
    }

    fn assert_proof(bytes: &[u8]) {
        let (root, account, key, value, storage) = ProofTuple::abi_decode(bytes).unwrap();
        let p = proof();
        assert_eq!(root, p.storage_root);
        let to_vecs = |v: Vec<Bytes>| v.into_iter().map(|b| b.to_vec()).collect::<Vec<_>>();
        assert_eq!(to_vecs(account), p.account_proof);
        assert_eq!(key, p.storage_key);
        assert_eq!(value, p.storage_value);
        assert_eq!(to_vecs(storage), p.storage_proof);
    }

    #[test]
    fn encode_evm_proof_roundtrips_as_abi_tuple() {
        assert_proof(&encode_evm_proof(&proof()));
    }

    #[tokio::test]
    async fn recv_packet_message() {
        let msg = offline_chain()
            .build_receive_packet_message(&packet(), proof(), EvmHeight(99), 3)
            .await
            .unwrap();
        assert_eq!(msg.to, ROUTER);
        assert_eq!(msg.value, U256::ZERO);
        let call = ICS26Router::recvPacketCall::abi_decode(&msg.calldata).unwrap();
        assert_packet(&call.msg_.packet);
        assert_proof(&call.msg_.proofCommitment);
        assert_eq!(call.msg_.proofHeight.revisionNumber, 3);
        assert_eq!(call.msg_.proofHeight.revisionHeight, 99);
    }

    #[tokio::test]
    async fn ack_packet_message() {
        let ack = EvmAcknowledgement(vec![7, 7, 7]);
        let msg = offline_chain()
            .build_ack_packet_message(&packet(), &ack, proof(), EvmHeight(5), 0)
            .await
            .unwrap();
        let call = ICS26Router::ackPacketCall::abi_decode(&msg.calldata).unwrap();
        assert_packet(&call.msg_.packet);
        assert_eq!(call.msg_.acknowledgement.as_ref(), &[7, 7, 7]);
        assert_proof(&call.msg_.proofAcked);
        assert_eq!(call.msg_.proofHeight.revisionHeight, 5);
    }

    #[tokio::test]
    async fn timeout_packet_message() {
        let msg = offline_chain()
            .build_timeout_packet_message(&packet(), proof(), EvmHeight(8), 1)
            .await
            .unwrap();
        let call = ICS26Router::timeoutPacketCall::abi_decode(&msg.calldata).unwrap();
        assert_packet(&call.msg_.packet);
        assert_proof(&call.msg_.proofTimeout);
        assert_eq!(call.msg_.proofHeight.revisionNumber, 1);
        assert_eq!(call.msg_.proofHeight.revisionHeight, 8);
    }
}
