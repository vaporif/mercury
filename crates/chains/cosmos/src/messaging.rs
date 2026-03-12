use async_trait::async_trait;
use tracing::{info, instrument, warn};

use mercury_chain_traits::messaging::CanSendMessages;
use mercury_chain_traits::tx::{CanPollTxResponse, CanQueryNonce, CanSubmitTx};
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::keys::CosmosSigner;

fn is_sequence_mismatch(e: &mercury_core::error::Error) -> bool {
    let msg = e.to_string();
    msg.contains("account sequence mismatch")
}

#[async_trait]
impl<S: CosmosSigner> CanSendMessages for CosmosChain<S> {
    #[instrument(skip_all, name = "send_messages", fields(count = messages.len()))]
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

        match self
            .submit_tx(&self.signer, &nonce, &fee, messages.clone())
            .await
        {
            Ok(tx_hash) => {
                info!("tx submitted: {tx_hash}");
                let response = self.poll_tx_response(&tx_hash).await?;
                Ok(vec![response])
            }
            Err(e) if is_sequence_mismatch(&e) => {
                warn!("sequence mismatch, refreshing nonce and retrying");
                let nonce = self.query_nonce(&self.signer).await?;
                let fee = self
                    .estimate_fee_with_nonce(&self.signer, &nonce, &messages)
                    .await?;
                let tx_hash = self.submit_tx(&self.signer, &nonce, &fee, messages).await?;
                info!("tx submitted on retry: {tx_hash}");
                let response = self.poll_tx_response(&tx_hash).await?;
                Ok(vec![response])
            }
            Err(e) => Err(e),
        }
    }
}
