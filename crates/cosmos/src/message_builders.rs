use async_trait::async_trait;
use prost::Message;

use mercury_chain_traits::message_builders::{
    CanBuildCreateClientMessage, CanBuildUpdateClientMessage, CanRegisterCounterparty,
};
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::payload_builders::{CosmosCreateClientPayload, CosmosUpdateClientPayload};
use crate::types::CosmosMessage;

/// IBC v2 `MsgCreateClient` — not yet in ibc-proto 0.52, so we define it manually.
#[derive(Clone, PartialEq, Message)]
struct MsgCreateClient {
    #[prost(string, tag = "1")]
    client_type: String,
    #[prost(bytes = "vec", tag = "2")]
    client_state: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    consensus_state: Vec<u8>,
    #[prost(string, tag = "4")]
    signer: String,
}

/// IBC v2 `MsgUpdateClient` — not yet in ibc-proto 0.52, so we define it manually.
#[derive(Clone, PartialEq, Message)]
struct MsgUpdateClient {
    #[prost(string, tag = "1")]
    client_id: String,
    #[prost(bytes = "vec", tag = "2")]
    client_message: Vec<u8>,
    #[prost(string, tag = "3")]
    signer: String,
}

/// IBC v2 `MsgRegisterCounterparty` — not yet in ibc-proto 0.52, so we define it manually.
#[derive(Clone, PartialEq, Message)]
struct MsgRegisterCounterparty {
    #[prost(string, tag = "1")]
    client_id: String,
    #[prost(string, tag = "2")]
    counterparty_client_id: String,
    #[prost(string, tag = "3")]
    signer: String,
}

fn encode_v2_msg(type_url: &str, msg: &impl Message) -> CosmosMessage {
    CosmosMessage {
        type_url: type_url.to_string(),
        value: msg.encode_to_vec(),
    }
}

#[async_trait]
impl CanBuildCreateClientMessage<Self> for CosmosChain {
    async fn build_create_client_message(
        &self,
        payload: CosmosCreateClientPayload,
    ) -> Result<CosmosMessage> {
        let signer = self.signer.account_address()?;

        let msg = MsgCreateClient {
            client_type: "07-tendermint".to_string(),
            client_state: payload.client_state_bytes,
            consensus_state: payload.consensus_state_bytes,
            signer,
        };

        Ok(encode_v2_msg("/ibc.core.client.v2.MsgCreateClient", &msg))
    }
}

#[async_trait]
impl CanBuildUpdateClientMessage<Self> for CosmosChain {
    async fn build_update_client_message(
        &self,
        client_id: &Self::ClientId,
        payload: CosmosUpdateClientPayload,
    ) -> Result<Vec<CosmosMessage>> {
        let signer = self.signer.account_address()?;

        let messages = payload
            .headers
            .into_iter()
            .map(|header_bytes| {
                let msg = MsgUpdateClient {
                    client_id: client_id.to_string(),
                    client_message: header_bytes,
                    signer: signer.clone(),
                };
                encode_v2_msg("/ibc.core.client.v2.MsgUpdateClient", &msg)
            })
            .collect();

        Ok(messages)
    }
}

#[async_trait]
impl CanRegisterCounterparty<Self> for CosmosChain {
    async fn build_register_counterparty_message(
        &self,
        client_id: &Self::ClientId,
        counterparty_client_id: &Self::ClientId,
    ) -> Result<CosmosMessage> {
        let signer = self.signer.account_address()?;

        let msg = MsgRegisterCounterparty {
            client_id: client_id.to_string(),
            counterparty_client_id: counterparty_client_id.to_string(),
            signer,
        };

        Ok(encode_v2_msg(
            "/ibc.core.client.v2.MsgRegisterCounterparty",
            &msg,
        ))
    }
}
