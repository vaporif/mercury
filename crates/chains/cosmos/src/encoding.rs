use prost::Message;

use crate::types::CosmosMessage;

/// Encode a protobuf message into a [`CosmosMessage`] with its type URL.
pub fn to_any<M: prost::Name + Message>(msg: &M) -> CosmosMessage {
    CosmosMessage {
        type_url: M::type_url(),
        value: msg.encode_to_vec(),
    }
}
