use borsh::{BorshDeserialize, BorshSerialize};

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub enum Delivery {
    Inline { data: Vec<u8> },
    Chunked { total_chunks: u8 },
}

impl Delivery {
    #[must_use]
    pub const fn total_chunks(&self) -> u8 {
        match self {
            Self::Inline { .. } => 0,
            Self::Chunked { total_chunks } => *total_chunks,
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct MsgPayload {
    pub source_port: String,
    pub dest_port: String,
    pub version: String,
    pub encoding: String,
    pub data: Delivery,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct MsgProof {
    pub height: u64,
    pub data: Delivery,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct MsgPacket {
    pub sequence: u64,
    pub source_client: String,
    pub dest_client: String,
    pub timeout_timestamp: u64,
    pub payloads: Vec<MsgPayload>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct Packet {
    pub sequence: u64,
    pub source_client: String,
    pub dest_client: String,
    pub timeout_timestamp: u64,
    pub payloads: Vec<Payload>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct Payload {
    pub source_port: String,
    pub dest_port: String,
    pub version: String,
    pub encoding: String,
    pub value: Vec<u8>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct MsgRecvPacket {
    pub packet: MsgPacket,
    pub proof: MsgProof,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct MsgAckPacket {
    pub packet: MsgPacket,
    pub acknowledgement: Vec<u8>,
    pub proof: MsgProof,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct MsgTimeoutPacket {
    pub packet: MsgPacket,
    pub proof: MsgProof,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct SignatureData {
    pub signature_hash: [u8; 32],
    pub pubkey: [u8; 32],
    pub msg: Vec<u8>,
    pub signature: [u8; 64],
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct IbcHeight {
    pub revision_number: u64,
    pub revision_height: u64,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct ClientState {
    pub chain_id: String,
    pub trust_level_numerator: u64,
    pub trust_level_denominator: u64,
    pub trusting_period: u64,
    pub unbonding_period: u64,
    pub max_clock_drift: u64,
    pub frozen_height: IbcHeight,
    pub latest_height: IbcHeight,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct ConsensusState {
    pub timestamp: u64,
    pub root: [u8; 32],
    pub next_validators_hash: [u8; 32],
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct CounterpartyInfo {
    pub client_id: String,
    pub merkle_prefix: Vec<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_recv_packet_round_trip() {
        let msg = MsgRecvPacket {
            packet: MsgPacket {
                sequence: 1,
                source_client: "07-tendermint-0".into(),
                dest_client: "07-tendermint-1".into(),
                timeout_timestamp: 1_000_000,
                payloads: vec![MsgPayload {
                    source_port: "transfer".into(),
                    dest_port: "transfer".into(),
                    version: "ics20-1".into(),
                    encoding: "proto3".into(),
                    data: Delivery::Inline {
                        data: vec![1, 2, 3],
                    },
                }],
            },
            proof: MsgProof {
                height: 100,
                data: Delivery::Inline {
                    data: vec![4, 5, 6],
                },
            },
        };
        let bytes = borsh::to_vec(&msg).unwrap();
        let decoded: MsgRecvPacket = borsh::from_slice(&bytes).unwrap();
        assert_eq!(decoded.packet.sequence, 1);
        assert_eq!(decoded.proof.height, 100);
    }

    #[test]
    fn signature_data_round_trip() {
        let sig = SignatureData {
            signature_hash: [1u8; 32],
            pubkey: [2u8; 32],
            msg: vec![3, 4, 5],
            signature: [6u8; 64],
        };
        let bytes = borsh::to_vec(&sig).unwrap();
        let decoded: SignatureData = borsh::from_slice(&bytes).unwrap();
        assert_eq!(decoded.pubkey, [2u8; 32]);
    }
}
