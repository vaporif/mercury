use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};

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
