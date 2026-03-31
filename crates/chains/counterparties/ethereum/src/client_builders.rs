use mercury_core::ThreadSafeAny;

use async_trait::async_trait;
use mercury_chain_cache::CachedChain;
use mercury_chain_traits::builders::{ClientMessageBuilder, ClientPayloadBuilder};
use mercury_chain_traits::types::MessageSender;
use mercury_core::plugin::{AnyChain, ClientBuilder};

use mercury_cosmos::builders::{CosmosCreateClientPayload, CosmosUpdateClientPayload};
use mercury_cosmos::chain::CosmosChain;
use mercury_cosmos::client_types::CosmosClientState;
use mercury_cosmos::keys::Secp256k1KeyPair;
use mercury_cosmos_counterparties::CosmosAdapter;
use mercury_ethereum::builders::{
    CreateClientPayload as EvmCreateClientPayload, UpdateClientPayload as EvmUpdateClientPayload,
};
use mercury_ethereum::chain::EthereumChain;
use mercury_ethereum::types::{EvmClientState, EvmHeight};

use crate::plugin::extract_evm_client_id;
use crate::types::EvmClientId;
use crate::wrapper::EthereumAdapter;

type CosmosCached = CachedChain<CosmosAdapter<Secp256k1KeyPair>>;
type EthCached = CachedChain<EthereumAdapter>;

fn downcast_cosmos(chain: &AnyChain) -> eyre::Result<&CosmosCached> {
    (**chain)
        .downcast_ref::<CosmosCached>()
        .ok_or_else(|| eyre::eyre!("expected cosmos chain handle"))
}

fn downcast_eth(chain: &AnyChain) -> eyre::Result<&EthCached> {
    (**chain)
        .downcast_ref::<EthCached>()
        .ok_or_else(|| eyre::eyre!("expected ethereum chain handle"))
}

pub struct EthereumTendermintClientBuilder;

#[async_trait]
impl ClientBuilder for EthereumTendermintClientBuilder {
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
        let c = downcast_eth(host_chain)?;

        if c.inner().0.config.light_client_address.is_none() {
            eyre::bail!(
                "light_client_address must be set in config to create a client on Ethereum"
            );
        }

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

        extract_evm_client_id(&responses).map(|id| id.to_string())
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
        let c = downcast_eth(host_chain)?;
        let parsed_id = EvmClientId(client_id.to_string());

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

pub struct EthereumZkClientBuilder;

#[async_trait]
impl ClientBuilder for EthereumZkClientBuilder {
    async fn build_create_payload(&self, src_chain: &AnyChain) -> eyre::Result<Box<ThreadSafeAny>> {
        let eth = downcast_eth(src_chain)?;
        let payload = ClientPayloadBuilder::<EthereumChain>::build_create_client_payload(eth)
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
        let eth_payload = payload
            .downcast_ref::<EvmCreateClientPayload>()
            .ok_or_else(|| eyre::eyre!("expected ethereum CreateClientPayload"))?;

        let msg = ClientMessageBuilder::<EthereumChain>::build_create_client_message(
            c,
            eth_payload.clone(),
        )
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;

        let responses = c
            .inner()
            .0
            .send_messages_with_responses(vec![msg])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        mercury_cosmos_counterparties::plugin::extract_cosmos_client_id(&responses)
            .map(|id| id.to_string())
    }

    async fn build_update_payload(
        &self,
        src_chain: &AnyChain,
        trusted_height: u64,
        target_height: u64,
        _counterparty_client_state: Option<&ThreadSafeAny>,
    ) -> eyre::Result<Box<ThreadSafeAny>> {
        let eth = downcast_eth(src_chain)?;
        let trusted = EvmHeight(trusted_height);
        let target = EvmHeight(target_height);
        let placeholder = EvmClientState(vec![]);
        let payload = ClientPayloadBuilder::<EthereumChain>::build_update_client_payload(
            eth,
            &trusted,
            &target,
            &placeholder,
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

        let eth_payload = payload
            .downcast_ref::<EvmUpdateClientPayload>()
            .ok_or_else(|| eyre::eyre!("expected ethereum UpdateClientPayload"))?;

        let output = ClientMessageBuilder::<EthereumChain>::build_update_client_message(
            c,
            &parsed_id,
            eth_payload.clone(),
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
