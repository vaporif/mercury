use async_trait::async_trait;
use mercury_chain_traits::builders::{MisbehaviourDetector, MisbehaviourMessageBuilder};
use mercury_chain_traits::queries::MisbehaviourQuery;
use mercury_core::error::Result;

use crate::chain::EthereumChain;
use crate::types::{EvmClientState, EvmHeight, EvmMessage};

#[async_trait]
impl MisbehaviourDetector<Self> for EthereumChain {
    type UpdateHeader = ();
    type MisbehaviourEvidence = ();
    type CounterpartyClientState = EvmClientState;

    async fn check_for_misbehaviour(
        &self,
        _client_id: &<Self as mercury_chain_traits::types::ChainTypes>::ClientId,
        _update_header: &(),
        _client_state: &EvmClientState,
    ) -> Result<Option<()>> {
        Ok(None)
    }
}

#[async_trait]
impl MisbehaviourMessageBuilder<Self> for EthereumChain {
    type MisbehaviourEvidence = ();

    async fn build_misbehaviour_message(
        &self,
        _client_id: &<Self as mercury_chain_traits::types::ChainTypes>::ClientId,
        _evidence: (),
    ) -> Result<EvmMessage> {
        eyre::bail!("Ethereum misbehaviour detection not yet implemented")
    }
}

#[async_trait]
impl MisbehaviourQuery<Self> for EthereumChain {
    type CounterpartyUpdateHeader = ();

    async fn query_consensus_state_heights(
        &self,
        _client_id: &<Self as mercury_chain_traits::types::ChainTypes>::ClientId,
    ) -> Result<Vec<EvmHeight>> {
        Ok(vec![])
    }

    async fn query_update_client_header(
        &self,
        _client_id: &<Self as mercury_chain_traits::types::ChainTypes>::ClientId,
        _consensus_height: &EvmHeight,
    ) -> Result<Option<()>> {
        Ok(None)
    }
}
