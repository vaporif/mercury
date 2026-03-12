use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::{HasChainTypes, HasIbcTypes};

/// Builds the payload needed to create a client on the counterparty chain.
#[async_trait]
pub trait CanBuildCreateClientPayload<Counterparty: HasChainTypes + ?Sized>: HasChainTypes {
    type CreateClientPayload: ThreadSafe;
    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload>;
}

/// Builds the payload needed to update a client between two heights.
#[async_trait]
pub trait CanBuildUpdateClientPayload<Counterparty: HasChainTypes + ?Sized>:
    HasIbcTypes<Counterparty>
{
    type UpdateClientPayload: ThreadSafe;
    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
    ) -> Result<Self::UpdateClientPayload>;
}
