use async_trait::async_trait;

use mercury_chain_traits::builders::ClientPayloadBuilder;
use mercury_core::error::Result;

use mercury_ethereum::chain::EthereumChainInner;
use mercury_ethereum::config::EthereumChainConfig;

/// Wrapper around `EthereumChainInner` that is local to this crate,
/// enabling cross-chain trait impls without orphan rule violations.
#[derive(Clone, Debug)]
pub struct EthereumChain(pub EthereumChainInner);

impl EthereumChain {
    pub async fn new(
        config: EthereumChainConfig,
        signer: alloy::signers::local::PrivateKeySigner,
    ) -> Result<Self> {
        EthereumChainInner::new(config, signer).await.map(Self)
    }
}

mercury_chain_traits::delegate_chain_inner! {
    impl[] EthereumChain => EthereumChainInner; skip_cpb
}

#[async_trait]
impl ClientPayloadBuilder<EthereumChainInner> for EthereumChain {
    type CreateClientPayload =
        <EthereumChainInner as ClientPayloadBuilder<EthereumChainInner>>::CreateClientPayload;
    type UpdateClientPayload =
        <EthereumChainInner as ClientPayloadBuilder<EthereumChainInner>>::UpdateClientPayload;

    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload> {
        self.0.build_create_client_payload().await
    }

    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
        counterparty_client_state: &<EthereumChainInner as mercury_chain_traits::IbcTypes>::ClientState,
    ) -> Result<Self::UpdateClientPayload> {
        self.0
            .build_update_client_payload(trusted_height, target_height, counterparty_client_state)
            .await
    }
}
