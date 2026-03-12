use async_trait::async_trait;

use mercury_chain_traits::payload_builders::{
    CanBuildCreateClientPayload, CanBuildUpdateClientPayload,
};
use mercury_core::error::Result;

use crate::chain::CosmosChain;

#[derive(Clone, Debug)]
pub struct CosmosCreateClientPayload {
    pub client_state_bytes: Vec<u8>,
    pub consensus_state_bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct CosmosUpdateClientPayload {
    pub headers: Vec<Vec<u8>>,
}

#[async_trait]
impl CanBuildCreateClientPayload<Self> for CosmosChain {
    type CreateClientPayload = CosmosCreateClientPayload;

    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload> {
        // TODO: fetch latest block + validators, encode as tendermint client/consensus state
        todo!("build create client payload")
    }
}

#[async_trait]
impl CanBuildUpdateClientPayload<Self> for CosmosChain {
    type UpdateClientPayload = CosmosUpdateClientPayload;

    async fn build_update_client_payload(
        &self,
        _trusted_height: &Self::Height,
        _target_height: &Self::Height,
    ) -> Result<Self::UpdateClientPayload> {
        // TODO: fetch headers between trusted and target, verify light client
        todo!("build update client payload")
    }
}
