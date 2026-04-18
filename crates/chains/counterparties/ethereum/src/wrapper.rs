use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use mercury_chain_traits::builders::ClientPayloadBuilder;
use mercury_core::error::Result;

use mercury_cosmos::misbehaviour::OnChainTmConsensusState;
use mercury_ethereum::chain::EthereumChain;
use mercury_ethereum::config::EthereumChainConfig;

pub type ConsensusStateCache = Arc<Mutex<HashMap<u64, OnChainTmConsensusState>>>;

#[derive(Clone)]
pub struct EthereumAdapter(pub EthereumChain, pub ConsensusStateCache);

impl EthereumAdapter {
    pub async fn new(
        config: EthereumChainConfig,
        signer: alloy::signers::local::PrivateKeySigner,
    ) -> Result<Self> {
        let chain = EthereumChain::new(config, signer).await?;
        Ok(Self(chain, Arc::new(Mutex::new(HashMap::new()))))
    }
}

impl std::fmt::Debug for EthereumAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

mercury_chain_traits::delegate_chain! {
    impl[] EthereumAdapter => EthereumChain; skip_cpb
}

#[async_trait]
impl ClientPayloadBuilder<EthereumChain> for EthereumAdapter {
    type CreateClientPayload =
        <EthereumChain as ClientPayloadBuilder<EthereumChain>>::CreateClientPayload;
    type UpdateClientPayload =
        <EthereumChain as ClientPayloadBuilder<EthereumChain>>::UpdateClientPayload;

    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload> {
        self.0.build_create_client_payload().await
    }

    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
        counterparty_client_state: &<EthereumChain as mercury_chain_traits::IbcTypes>::ClientState,
    ) -> Result<Self::UpdateClientPayload> {
        self.0
            .build_update_client_payload(trusted_height, target_height, counterparty_client_state)
            .await
    }

    fn update_payload_proof_height(
        &self,
        payload: &Self::UpdateClientPayload,
    ) -> Option<Self::Height> {
        self.0.update_payload_proof_height(payload)
    }

    fn update_payload_message_height(
        &self,
        payload: &Self::UpdateClientPayload,
    ) -> Option<Self::Height> {
        self.0.update_payload_message_height(payload)
    }
}
