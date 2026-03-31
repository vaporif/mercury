use mercury_core::ThreadSafeAny;

use async_trait::async_trait;
use mercury_chain_traits::builders::{ClientMessageBuilder, ClientPayloadBuilder};
use mercury_chain_traits::types::MessageSender;
use mercury_core::plugin::{AnyChain, ClientBuilder};

use crate::builders::{CosmosCreateClientPayload, CosmosUpdateClientPayload};
use crate::chain::CosmosChain;
use crate::client_types::CosmosClientState;
use crate::keys::Secp256k1KeyPair;
use crate::plugin::{downcast_cosmos, extract_cosmos_client_id};
use crate::wrapper::CosmosAdapter;

pub struct CosmosTendermintClientBuilder;

#[async_trait]
impl ClientBuilder for CosmosTendermintClientBuilder {
    async fn build_create_payload(&self, src_chain: &AnyChain) -> eyre::Result<Box<ThreadSafeAny>> {
        let c = downcast_cosmos(src_chain)?;
        let payload =
            ClientPayloadBuilder::<CosmosAdapter<Secp256k1KeyPair>>::build_create_client_payload(c)
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
        Ok(Box::new(payload))
    }

    async fn create_client(
        &self,
        host_chain: &AnyChain,
        payload: Box<ThreadSafeAny>,
    ) -> eyre::Result<String> {
        let c = downcast_cosmos(host_chain)?;
        let cosmos_payload = payload
            .downcast_ref::<CosmosCreateClientPayload>()
            .ok_or_else(|| eyre::eyre!("expected CosmosCreateClientPayload"))?;

        let msg =
            ClientMessageBuilder::<CosmosChain<Secp256k1KeyPair>>::build_create_client_message(
                c,
                cosmos_payload.clone(),
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        let responses = c
            .inner()
            .0
            .send_messages_with_responses(vec![msg])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        Ok(extract_cosmos_client_id(&responses)?.to_string())
    }

    async fn build_update_payload(
        &self,
        src_chain: &AnyChain,
        trusted_height: u64,
        target_height: u64,
        _counterparty_client_state: Option<&ThreadSafeAny>,
    ) -> eyre::Result<Box<ThreadSafeAny>> {
        let c = downcast_cosmos(src_chain)?;
        let trusted = tendermint::block::Height::try_from(trusted_height)?;
        let target = tendermint::block::Height::try_from(target_height)?;

        let payload =
            ClientPayloadBuilder::<CosmosAdapter<Secp256k1KeyPair>>::build_update_client_payload(
                c,
                &trusted,
                &target,
                &CosmosClientState::placeholder(),
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        Ok(Box::new(payload))
    }

    async fn update_client(
        &self,
        host_chain: &AnyChain,
        client_id: &str,
        payload: Box<ThreadSafeAny>,
    ) -> eyre::Result<()> {
        let c = downcast_cosmos(host_chain)?;
        let parsed_id: ibc::core::host::types::identifiers::ClientId = client_id
            .parse()
            .map_err(|e| eyre::eyre!("invalid cosmos client ID '{client_id}': {e}"))?;

        let cosmos_payload = payload
            .downcast_ref::<CosmosUpdateClientPayload>()
            .ok_or_else(|| eyre::eyre!("expected CosmosUpdateClientPayload"))?;

        let output =
            ClientMessageBuilder::<CosmosChain<Secp256k1KeyPair>>::build_update_client_message(
                c,
                &parsed_id,
                cosmos_payload.clone(),
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        c.inner()
            .0
            .send_messages(output.messages)
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        Ok(())
    }
}
