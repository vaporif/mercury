use async_trait::async_trait;
use tracing::{info, warn};

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

        let msg_count = messages.len();
        let mut retry = false;

        loop {
            let mut nonce_guard = self.nonce_mutex.lock().await;

            let nonce = if let Some(n) = nonce_guard.as_ref() {
                n.clone()
            } else {
                let n = self.query_nonce(&self.signer).await?;
                *nonce_guard = Some(n.clone());
                n
            };

            let fee = self.estimate_fee(&self.signer, &messages).await?;

            let tx_hash = match self
                .submit_tx(&self.signer, &nonce, &fee, messages.clone())
                .await
            {
                Ok(hash) => hash,
                Err(e) => {
                    let err_str = format!("{e:?}");
                    if !retry && err_str.contains("sequence") {
                        warn!("sequence mismatch, re-querying nonce");
                        *nonce_guard = None;
                        drop(nonce_guard);
                        retry = true;
                        continue;
                    }
                    return Err(e);
                }
            };

            info!("tx submitted: {tx_hash}");

            let response = self.poll_tx_response(&tx_hash).await?;

            if let Some(ref mut n) = *nonce_guard {
                n.sequence += 1;
            }

            drop(nonce_guard);

            let responses: Vec<Self::MessageResponse> =
                std::iter::repeat_n(response, msg_count).collect();

            return Ok(responses);
        }
    }
}
