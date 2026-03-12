use async_trait::async_trait;
use tracing::info;

use mercury_chain_traits::messaging::CanSendMessages;
use mercury_chain_traits::tx::{CanPollTxResponse, CanQueryNonce, CanSubmitTx};
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::keys::CosmosSigner;

#[async_trait]
impl<S: CosmosSigner> CanSendMessages for CosmosChain<S> {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<Vec<Self::MessageResponse>> {
        if messages.is_empty() {
            return Ok(vec![]);
        }

        let nonce = self.query_nonce(&self.signer).await?;
        let fee = self
            .estimate_fee_with_nonce(&self.signer, &nonce, &messages)
            .await?;
        let tx_hash = self.submit_tx(&self.signer, &nonce, &fee, messages).await?;

        info!("tx submitted: {tx_hash}");

        let response = self.poll_tx_response(&tx_hash).await?;

        Ok(vec![response])
    }
}
