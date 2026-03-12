use async_trait::async_trait;
use tracing::info;

use mercury_chain_traits::messaging::CanSendMessages;
use mercury_chain_traits::tx::{CanEstimateFee, CanPollTxResponse, CanQueryNonce, CanSubmitTx};
use mercury_core::error::Result;

use crate::chain::CosmosChain;

#[async_trait]
impl CanSendMessages for CosmosChain {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<Vec<Self::MessageResponse>> {
        if messages.is_empty() {
            return Ok(vec![]);
        }

        // Nonce is queried fresh each time rather than cached. This is safe because
        // TxWorker is the sole submitter per chain, so there are no concurrent submissions
        // that could cause sequence conflicts.
        let nonce = self.query_nonce(&self.signer).await?;
        let fee = self.estimate_fee(&self.signer, &messages).await?;
        let tx_hash = self.submit_tx(&self.signer, &nonce, &fee, messages).await?;

        info!("tx submitted: {tx_hash}");

        let response = self.poll_tx_response(&tx_hash).await?;

        Ok(vec![response])
    }
}
