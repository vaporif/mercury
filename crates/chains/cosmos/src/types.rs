use ibc::core::host::types::identifiers::ClientId;
use tendermint::block::Height as TmHeight;

#[derive(Clone, Debug)]
pub struct CosmosEvent {
    pub kind: String,
    pub attributes: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct CosmosMessage {
    pub type_url: String,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct CosmosTxResponse {
    pub hash: String,
    pub height: TmHeight,
    pub events: Vec<CosmosEvent>,
}

#[derive(Clone, Debug)]
pub struct CosmosChainStatus {
    pub height: TmHeight,
    pub timestamp: tendermint::Time,
}

#[derive(Clone, Debug)]
pub struct MerkleProof {
    pub proof_bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct CosmosPacket {
    pub source_client_id: ClientId,
    pub dest_client_id: ClientId,
    pub sequence: u64,
    pub timeout_timestamp: u64,
    pub payloads: Vec<PacketPayload>,
}

#[derive(Clone, Debug)]
pub struct PacketPayload {
    pub source_port: String,
    pub dest_port: String,
    pub version: String,
    pub encoding: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct PacketCommitment(pub Vec<u8>);

#[derive(Clone, Debug)]
pub struct PacketReceipt;

#[derive(Clone, Debug)]
pub struct PacketAcknowledgement(pub Vec<u8>);

#[derive(Clone, Debug)]
pub struct SendPacketEvent {
    pub packet: CosmosPacket,
}

#[derive(Clone, Debug)]
pub struct WriteAckEvent {
    pub packet: CosmosPacket,
    pub ack: PacketAcknowledgement,
}
