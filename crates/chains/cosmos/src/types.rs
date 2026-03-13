use ibc::core::host::types::identifiers::ClientId;
use prost::Message;
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

/// Encode a protobuf message into a [`CosmosMessage`] with its type URL.
#[must_use]
pub fn to_any<M: prost::Name + Message>(msg: &M) -> CosmosMessage {
    CosmosMessage {
        type_url: M::type_url(),
        value: msg.encode_to_vec(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_any_encodes_correctly() {
        use crate::ibc_v2::client::MsgRegisterCounterparty;
        use prost::{Message, Name};

        let msg = MsgRegisterCounterparty {
            client_id: "07-tendermint-0".to_string(),
            counterparty_merkle_prefix: vec![b"ibc".to_vec()],
            counterparty_client_id: "07-tendermint-1".to_string(),
            signer: "cosmos1abc".to_string(),
        };

        let result = to_any(&msg);
        assert_eq!(result.type_url, MsgRegisterCounterparty::type_url());
        assert_eq!(result.value, msg.encode_to_vec());
    }
}
