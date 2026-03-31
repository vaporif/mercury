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
use mercury_solana::types::SolanaClientId;

use crate::plugin::downcast_solana;
use crate::wrapper::SolanaAdapter;

type CosmosCached = CachedChain<CosmosAdapter<Secp256k1KeyPair>>;

fn downcast_cosmos(chain: &AnyChain) -> eyre::Result<&CosmosCached> {
    (**chain)
        .downcast_ref::<CosmosCached>()
        .ok_or_else(|| eyre::eyre!("expected cosmos chain handle"))
}

pub struct SolanaTendermintClientBuilder;

#[async_trait]
impl ClientBuilder for SolanaTendermintClientBuilder {
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
        let solana = downcast_solana(host_chain)?;
        let cosmos_payload = payload
            .downcast_ref::<CosmosCreateClientPayload>()
            .ok_or_else(|| eyre::eyre!("expected CosmosCreateClientPayload"))?;

        let msg = <CachedChain<SolanaAdapter> as ClientMessageBuilder<
            CosmosChain<Secp256k1KeyPair>,
        >>::build_create_client_message(solana, cosmos_payload.clone())
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;

        solana
            .inner()
            .0
            .send_messages(vec![msg])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        eyre::bail!(
            "Solana create_client: client ID extraction not yet implemented -- \
             requires Solana IBC program log parsing (blocked on send_messages impl)"
        )
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
        let solana = downcast_solana(host_chain)?;
        let solana_client_id = SolanaClientId(client_id.to_string());
        let cosmos_payload = payload
            .downcast_ref::<CosmosUpdateClientPayload>()
            .ok_or_else(|| eyre::eyre!("expected CosmosUpdateClientPayload"))?;

        let output = <CachedChain<SolanaAdapter> as ClientMessageBuilder<
            CosmosChain<Secp256k1KeyPair>,
        >>::build_update_client_message(
            solana, &solana_client_id, cosmos_payload.clone()
        )
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;

        solana
            .inner()
            .0
            .send_messages(output.messages)
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        Ok(())
    }
}

pub struct SolanaNativeClientBuilder;

#[async_trait]
impl ClientBuilder for SolanaNativeClientBuilder {
    async fn build_create_payload(
        &self,
        _src_chain: &AnyChain,
    ) -> eyre::Result<Box<ThreadSafeAny>> {
        eyre::bail!("solana native source payload must be built via the solana-side builder")
    }

    async fn create_client(
        &self,
        host_chain: &AnyChain,
        payload: Box<ThreadSafeAny>,
    ) -> eyre::Result<String> {
        let c = downcast_cosmos(host_chain)?;
        let solana_payload = payload
            .downcast_ref::<mercury_solana::types::SolanaCreateClientPayload>()
            .ok_or_else(|| eyre::eyre!("expected SolanaCreateClientPayload"))?;

        let msg =
            ClientMessageBuilder::<mercury_solana::chain::SolanaChain>::build_create_client_message(
                c,
                solana_payload.clone(),
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
        _src_chain: &AnyChain,
        _trusted_height: u64,
        _target_height: u64,
        _counterparty_client_state: Option<&ThreadSafeAny>,
    ) -> eyre::Result<Box<ThreadSafeAny>> {
        eyre::bail!("solana native source payload must be built via the solana-side builder")
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

        let solana_payload = payload
            .downcast_ref::<mercury_solana::types::SolanaUpdateClientPayload>()
            .ok_or_else(|| eyre::eyre!("expected SolanaUpdateClientPayload"))?;

        let output =
            ClientMessageBuilder::<mercury_solana::chain::SolanaChain>::build_update_client_message(
                c,
                &parsed_id,
                solana_payload.clone(),
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
