use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};

/// Solana slot number used as block height.
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

/// Unix timestamp in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SolanaTimestamp(pub u64);

/// Solana chain identifier (e.g., mainnet-beta, devnet).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SolanaChainId(pub String);

impl std::fmt::Display for SolanaChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// IBC client identifier on Solana.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SolanaClientId(pub String);

impl std::fmt::Display for SolanaClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Raw on-chain event from Solana program logs.
#[derive(Clone, Debug)]
pub struct SolanaEvent {
    pub program_id: String,
    pub data: Vec<u8>,
}

/// Instruction to submit to the Solana IBC program.
#[derive(Clone, Debug)]
pub struct SolanaMessage {
    pub program_id: String,
    pub data: Vec<u8>,
}

/// Response from a confirmed Solana transaction.
#[derive(Clone, Debug)]
pub struct SolanaTxResponse {
    pub signature: String,
    pub slot: u64,
}

/// Chain status snapshot.
#[derive(Clone, Debug)]
pub struct SolanaChainStatus {
    pub height: SolanaHeight,
    pub timestamp: SolanaTimestamp,
}

/// Opaque client state bytes.
#[derive(Clone, Debug)]
pub struct SolanaClientState(pub Vec<u8>);

/// Opaque consensus state bytes.
#[derive(Clone, Debug)]
pub struct SolanaConsensusState(pub Vec<u8>);

/// Merkle proof for Solana state.
#[derive(Clone, Debug)]
pub struct SolanaCommitmentProof(pub Vec<u8>);

/// IBC packet on Solana.
#[derive(Clone, Debug)]
pub struct SolanaPacket {
    pub source_client_id: String,
    pub dest_client_id: String,
    pub sequence: PacketSequence,
    pub timeout_timestamp: TimeoutTimestamp,
    pub payloads: Vec<SolanaPayload>,
}

/// Payload within a Solana IBC packet.
#[derive(Clone, Debug)]
pub struct SolanaPayload {
    pub source_port: Port,
    pub dest_port: Port,
    pub version: String,
    pub encoding: String,
    pub data: Vec<u8>,
}

/// Packet commitment hash.
#[derive(Clone, Debug)]
pub struct SolanaPacketCommitment(pub Vec<u8>);

/// Packet receipt marker.
#[derive(Clone, Debug)]
pub struct SolanaPacketReceipt;

/// Acknowledgement bytes.
#[derive(Clone, Debug)]
pub struct SolanaAcknowledgement(pub Vec<u8>);

/// Send packet event.
#[derive(Clone, Debug)]
pub struct SendPacketEvent {
    pub packet: SolanaPacket,
}

/// Write acknowledgement event.
#[derive(Clone, Debug)]
pub struct WriteAckEvent {
    pub packet: SolanaPacket,
    pub ack: SolanaAcknowledgement,
}

/// Create client payload.
#[derive(Clone, Debug)]
pub struct SolanaCreateClientPayload {
    pub client_state: Vec<u8>,
    pub consensus_state: Vec<u8>,
}

/// Update client payload.
#[derive(Clone, Debug)]
pub struct SolanaUpdateClientPayload {
    pub headers: Vec<Vec<u8>>,
}
