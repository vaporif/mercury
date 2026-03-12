use async_trait::async_trait;

use ibc_proto::ibc::core::client::v1::{MsgCreateClient, MsgUpdateClient};
use mercury_chain_traits::message_builders::{
    CanBuildCreateClientMessage, CanBuildUpdateClientMessage, CanRegisterCounterparty,
};
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::encoding::to_any;
use crate::ibc_v2::client::MsgRegisterCounterparty;
use crate::payload_builders::{CosmosCreateClientPayload, CosmosUpdateClientPayload};
use crate::types::CosmosMessage;

/// Default IBC store merkle prefix used by Cosmos SDK chains.
const DEFAULT_MERKLE_PREFIX: &[&[u8]] = &[b"ibc", b""];

#[async_trait]
impl CanBuildCreateClientMessage<Self> for CosmosChain {
    async fn build_create_client_message(
        &self,
        payload: CosmosCreateClientPayload,
    ) -> Result<CosmosMessage> {
        let signer = self.signer.account_address()?;

        let msg = MsgCreateClient {
            client_state: Some(payload.client_state),
            consensus_state: Some(payload.consensus_state),
            signer,
        };

        Ok(to_any(&msg))
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
            .map(|header_any| {
                let msg = MsgUpdateClient {
                    client_id: client_id.to_string(),
                    client_message: Some(header_any),
                    signer: signer.clone(),
                };
                to_any(&msg)
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
            counterparty_merkle_prefix: DEFAULT_MERKLE_PREFIX.iter().map(|s| s.to_vec()).collect(),
            counterparty_client_id: counterparty_client_id.to_string(),
            signer,
        };

        Ok(to_any(&msg))
    }
}
