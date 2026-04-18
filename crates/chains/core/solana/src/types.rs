use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};

use crate::ibc_types;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SolanaHeight(pub u64);

impl std::fmt::Display for SolanaHeight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<SolanaHeight> for u64 {
    fn from(h: SolanaHeight) -> Self {
        h.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SolanaTimestamp(pub u64);

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SolanaChainId;

impl std::fmt::Display for SolanaChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<n/a>")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SolanaClientId(pub String);

impl std::fmt::Display for SolanaClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug)]
pub struct SolanaEvent {
    pub program_id: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct SolanaMessage {
    pub instructions: Vec<solana_sdk::instruction::Instruction>,
}

#[derive(Clone, Debug)]
pub struct SolanaTxResponse {
    pub signature: String,
    pub slot: u64,
}

#[derive(Clone, Debug)]
pub struct SolanaChainStatus {
    pub height: SolanaHeight,
    pub timestamp: SolanaTimestamp,
}

#[derive(Clone, Debug)]
pub struct SolanaClientState(pub Vec<u8>);

#[derive(Clone, Debug)]
pub struct SolanaConsensusState(pub Vec<u8>);

#[derive(Clone, Debug)]
pub struct SolanaCommitmentProof(pub Vec<u8>);

#[derive(Clone, Debug)]
pub struct SolanaPacket {
    pub source_client_id: String,
    pub dest_client_id: String,
    pub sequence: PacketSequence,
    pub timeout_timestamp: TimeoutTimestamp,
    pub payloads: Vec<SolanaPayload>,
}

#[derive(Clone, Debug)]
pub struct SolanaPayload {
    pub source_port: Port,
    pub dest_port: Port,
    pub version: String,
    pub encoding: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct SolanaPacketCommitment(pub Vec<u8>);

#[derive(Clone, Debug)]
pub struct SolanaPacketReceipt;

#[derive(Clone, Debug)]
pub struct SolanaAcknowledgement(pub Vec<u8>);

#[derive(Clone, Debug)]
pub struct SendPacketEvent {
    pub packet: SolanaPacket,
}

#[derive(Clone, Debug)]
pub struct WriteAckEvent {
    pub packet: SolanaPacket,
    pub ack: SolanaAcknowledgement,
}

#[derive(Clone, Debug)]
pub struct SolanaCreateClientPayload {
    pub client_state: Vec<u8>,
    pub consensus_state: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct SolanaUpdateClientPayload {
    pub headers: Vec<Vec<u8>>,
}

impl SolanaPacket {
    /// Convert to on-chain `MsgPacket` with inline delivery.
    #[must_use]
    pub fn to_msg_packet(&self) -> ibc_types::MsgPacket {
        ibc_types::MsgPacket {
            sequence: self.sequence.0,
            source_client: self.source_client_id.clone(),
            dest_client: self.dest_client_id.clone(),
            timeout_timestamp: self.timeout_timestamp.0,
            payloads: self
                .payloads
                .iter()
                .map(|p| ibc_types::MsgPayload {
                    source_port: p.source_port.0.clone(),
                    dest_port: p.dest_port.0.clone(),
                    version: p.version.clone(),
                    encoding: p.encoding.clone(),
                    data: ibc_types::Delivery::Inline {
                        data: p.data.clone(),
                    },
                })
                .collect(),
        }
    }
}

impl From<SolanaPacket> for ibc_types::Packet {
    fn from(p: SolanaPacket) -> Self {
        Self {
            sequence: p.sequence.0,
            source_client: p.source_client_id,
            dest_client: p.dest_client_id,
            timeout_timestamp: p.timeout_timestamp.0,
            payloads: p.payloads.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<SolanaPayload> for ibc_types::Payload {
    fn from(p: SolanaPayload) -> Self {
        Self {
            source_port: p.source_port.0,
            dest_port: p.dest_port.0,
            version: p.version,
            encoding: p.encoding,
            value: p.data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solana_packet_to_msg_packet() {
        let packet = SolanaPacket {
            source_client_id: "07-tendermint-0".into(),
            dest_client_id: "07-tendermint-1".into(),
            sequence: PacketSequence(1),
            timeout_timestamp: TimeoutTimestamp(1_000_000),
            payloads: vec![SolanaPayload {
                source_port: Port("transfer".into()),
                dest_port: Port("transfer".into()),
                version: "ics20-1".into(),
                encoding: "proto3".into(),
                data: vec![1, 2, 3],
            }],
        };
        let msg = packet.to_msg_packet();
        assert_eq!(msg.sequence, 1);
        assert_eq!(msg.source_client, "07-tendermint-0");
        match &msg.payloads[0].data {
            ibc_types::Delivery::Inline { data } => assert_eq!(data, &[1, 2, 3]),
            _ => panic!("expected inline delivery"),
        }
    }
}
