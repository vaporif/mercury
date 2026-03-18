use prost::Message;
use tendermint::block::Height as TmHeight;

use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};

/// Protobuf type URL identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeUrl(pub String);

impl std::fmt::Display for TypeUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for TypeUrl {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<TypeUrl> for String {
    fn from(v: TypeUrl) -> Self {
        v.0
    }
}

impl AsRef<str> for TypeUrl {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// ABCI event
#[derive(Clone, Debug)]
pub struct CosmosEvent {
    pub kind: String,
    pub attributes: Vec<(String, String)>,
}

/// Protobuf-encoded message
#[derive(Clone, Debug)]
pub struct CosmosMessage {
    pub type_url: TypeUrl,
    pub value: Vec<u8>,
}

/// Encode a protobuf message into a [`CosmosMessage`] with its type URL.
#[must_use]
pub fn to_any<M: prost::Name + Message>(msg: &M) -> CosmosMessage {
    CosmosMessage {
        type_url: M::type_url().into(),
        value: msg.encode_to_vec(),
    }
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

/// Raw client identifier from an on-chain packet. Accepts any string,
/// unlike `ibc::ClientId` which enforces 9–64 char validation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RawClientId(pub String);

impl std::fmt::Display for RawClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for RawClientId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl AsRef<str> for RawClientId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// An IBC v2 packet with routing info and payloads.
#[derive(Clone, Debug)]
pub struct CosmosPacket {
    pub source_client_id: RawClientId,
    pub dest_client_id: RawClientId,
    pub sequence: PacketSequence,
    pub timeout_timestamp: TimeoutTimestamp,
    pub payloads: Vec<PacketPayload>,
}

/// A single payload within a packet, carrying application data.
#[derive(Clone, Debug)]
pub struct PacketPayload {
    pub source_port: Port,
    pub dest_port: Port,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_any_encodes_correctly() {
        use ibc_proto::ibc::core::client::v2::MsgRegisterCounterparty;
        use prost::{Message, Name};

        let msg = MsgRegisterCounterparty {
            client_id: "07-tendermint-0".to_string(),
            counterparty_merkle_prefix: vec![b"ibc".to_vec()],
            counterparty_client_id: "07-tendermint-1".to_string(),
            signer: "cosmos1abc".to_string(),
        };

        let result = to_any(&msg);
        assert_eq!(result.type_url, MsgRegisterCounterparty::type_url().into());
        assert_eq!(result.value, msg.encode_to_vec());
    }
}
