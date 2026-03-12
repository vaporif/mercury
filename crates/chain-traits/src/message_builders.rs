use async_trait::async_trait;
use mercury_core::error::Result;

use crate::payload_builders::{CanBuildCreateClientPayload, CanBuildUpdateClientPayload};
use crate::types::{HasChainTypes, HasIbcTypes, HasMessageTypes};

/// Builds a message to create a new IBC client.
#[async_trait]
pub trait CanBuildCreateClientMessage<Counterparty>: HasMessageTypes
where
    Counterparty: HasChainTypes + CanBuildCreateClientPayload<Self>,
{
    async fn build_create_client_message(
        &self,
        payload: Counterparty::CreateClientPayload,
    ) -> Result<Self::Message>;
}

/// Builds messages to update an existing IBC client.
#[async_trait]
pub trait CanBuildUpdateClientMessage<Counterparty>:
    HasMessageTypes + HasIbcTypes<Counterparty>
where
    Counterparty: HasChainTypes + CanBuildUpdateClientPayload<Self>,
{
    async fn build_update_client_message(
        &self,
        client_id: &Self::ClientId,
        payload: Counterparty::UpdateClientPayload,
    ) -> Result<Vec<Self::Message>>;
}

/// Builds a message to register a counterparty client mapping.
#[async_trait]
pub trait CanRegisterCounterparty<Counterparty>:
    HasMessageTypes + HasIbcTypes<Counterparty>
where
    Counterparty: HasChainTypes + HasIbcTypes<Self>,
{
    async fn build_register_counterparty_message(
        &self,
        client_id: &Self::ClientId,
        counterparty_client_id: &<Counterparty as HasIbcTypes<Self>>::ClientId,
    ) -> Result<Self::Message>;
}
