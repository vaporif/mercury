use ibc::core::host::types::identifiers::ClientId;
use tendermint::block::Height as TmHeight;

/// An ABCI event with a type string and key-value attributes.
#[derive(Clone, Debug)]
pub struct CosmosEvent {
    pub kind: String,
    pub attributes: Vec<(String, String)>,
}

/// A protobuf-encoded Cosmos SDK message (type URL + bytes).
#[derive(Clone, Debug)]
pub struct CosmosMessage {
    pub type_url: String,
    pub value: Vec<u8>,
}

/// Result of a confirmed transaction including hash, height, and events.
#[derive(Clone, Debug)]
pub struct CosmosTxResponse {
    pub hash: String,
    pub height: TmHeight,
    pub events: Vec<CosmosEvent>,
}

/// Latest block height and timestamp of a Cosmos chain.
#[derive(Clone, Debug)]
pub struct CosmosChainStatus {
    pub height: TmHeight,
    pub timestamp: tendermint::Time,
}

/// Raw Merkle proof bytes for IBC state verification.
#[derive(Clone, Debug)]
pub struct MerkleProof {
    pub proof_bytes: Vec<u8>,
}

/// An IBC v2 packet with routing info and payloads.
#[derive(Clone, Debug)]
pub struct CosmosPacket {
    pub source_client_id: ClientId,
    pub dest_client_id: ClientId,
    pub sequence: u64,
    pub timeout_timestamp: u64,
    pub payloads: Vec<PacketPayload>,
}

/// A single payload within a packet, carrying application data.
#[derive(Clone, Debug)]
pub struct PacketPayload {
    pub source_port: String,
    pub dest_port: String,
    pub version: String,
    pub encoding: String,
    pub data: Vec<u8>,
}

/// On-chain commitment hash for a sent packet.
#[derive(Clone, Debug)]
pub struct PacketCommitment(pub Vec<u8>);

/// Marker indicating a packet has been received.
#[derive(Clone, Debug)]
pub struct PacketReceipt;

/// Raw acknowledgement bytes for a received packet.
#[derive(Clone, Debug)]
pub struct PacketAcknowledgement(pub Vec<u8>);

/// Event emitted when a packet is sent from this chain.
#[derive(Clone, Debug)]
pub struct SendPacketEvent {
    pub packet: CosmosPacket,
}

/// Event emitted when an acknowledgement is written for a received packet.
#[derive(Clone, Debug)]
pub struct WriteAckEvent {
    pub packet: CosmosPacket,
    pub ack: PacketAcknowledgement,
}
