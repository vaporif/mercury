use async_trait::async_trait;
use mercury_core::error::Result;

use crate::payload_builders::CanBuildClientPayloads;
use crate::types::{HasChainTypes, HasIbcTypes, HasMessageTypes};

/// Builds messages for creating/updating IBC clients and registering counterparties.
#[async_trait]
pub trait CanBuildClientMessages<Counterparty>: HasMessageTypes + HasIbcTypes<Counterparty>
where
    Counterparty: HasChainTypes + CanBuildClientPayloads<Self> + HasIbcTypes<Self>,
{
    async fn build_create_client_message(
        &self,
        payload: Counterparty::CreateClientPayload,
    ) -> Result<Self::Message>;

    async fn build_update_client_message(
        &self,
        client_id: &Self::ClientId,
        payload: Counterparty::UpdateClientPayload,
    ) -> Result<Vec<Self::Message>>;

    async fn build_register_counterparty_message(
        &self,
        client_id: &Self::ClientId,
        counterparty_client_id: &<Counterparty as HasIbcTypes<Self>>::ClientId,
    ) -> Result<Self::Message>;
}
