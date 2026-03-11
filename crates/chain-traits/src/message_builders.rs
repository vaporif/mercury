use async_trait::async_trait;
use mercury_core::error::Result;

use crate::payload_builders::{CanBuildCreateClientPayload, CanBuildUpdateClientPayload};
use crate::types::{HasChainTypes, HasIbcTypes, HasMessageTypes};

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
