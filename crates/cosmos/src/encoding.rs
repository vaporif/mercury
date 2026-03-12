use prost::Message;

use crate::types::CosmosMessage;

pub fn to_any<M: prost::Name + Message>(msg: &M) -> CosmosMessage {
    CosmosMessage {
        type_url: M::type_url(),
        value: msg.encode_to_vec(),
    }
}
