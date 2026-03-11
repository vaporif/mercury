use async_trait::async_trait;

use mercury_chain_traits::message_builders::{
    CanBuildCreateClientMessage, CanBuildUpdateClientMessage, CanRegisterCounterparty,
};
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::payload_builders::{CosmosCreateClientPayload, CosmosUpdateClientPayload};
use crate::types::CosmosMessage;

#[async_trait]
impl CanBuildCreateClientMessage<CosmosChain> for CosmosChain {
    async fn build_create_client_message(
        &self,
        _payload: CosmosCreateClientPayload,
    ) -> Result<CosmosMessage> {
        // TODO: encode MsgCreateClient proto message
        todo!("build create client message")
    }
}

#[async_trait]
impl CanBuildUpdateClientMessage<CosmosChain> for CosmosChain {
    async fn build_update_client_message(
        &self,
        _client_id: &Self::ClientId,
        _payload: CosmosUpdateClientPayload,
    ) -> Result<Vec<CosmosMessage>> {
        // TODO: encode MsgUpdateClient proto messages (one per header)
        todo!("build update client messages")
    }
}

#[async_trait]
impl CanRegisterCounterparty<CosmosChain> for CosmosChain {
    async fn build_register_counterparty_message(
        &self,
        _client_id: &Self::ClientId,
        _counterparty_client_id: &Self::ClientId,
    ) -> Result<CosmosMessage> {
        // TODO: encode MsgRegisterCounterparty proto message
        todo!("build register counterparty message")
    }
}
