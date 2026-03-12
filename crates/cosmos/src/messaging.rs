use async_trait::async_trait;
use tracing::info;

use mercury_chain_traits::messaging::CanSendMessages;
use mercury_chain_traits::tx::{CanEstimateFee, CanPollTxResponse, CanQueryNonce, CanSubmitTx};
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

        // Nonce is queried fresh each time. TxWorker may call this concurrently
        // (up to MAX_IN_FLIGHT batches). Occasional sequence conflicts on
        // nonce-based chains are acceptable — the event rescan retries missed packets.
        let nonce = self.query_nonce(&self.signer).await?;
        let fee = self.estimate_fee(&self.signer, &messages).await?;
        let tx_hash = self.submit_tx(&self.signer, &nonce, &fee, messages).await?;

        info!("tx submitted: {tx_hash}");

        let response = self.poll_tx_response(&tx_hash).await?;

        Ok(vec![response])
    }
}
