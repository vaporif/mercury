use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::{HasChainTypes, HasIbcTypes};

/// Builds payloads for creating and updating IBC light clients.
#[async_trait]
pub trait CanBuildClientPayloads<Counterparty: HasChainTypes + ?Sized>: HasIbcTypes<Counterparty> {
    type CreateClientPayload: ThreadSafe;
    type UpdateClientPayload: ThreadSafe;

    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload>;

    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
    ) -> Result<Self::UpdateClientPayload>;
}
