use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use async_trait::async_trait;
use eyre::Context;
use mercury_chain_traits::types::MessageSender;
use mercury_core::error::Result;
use tracing::{info, warn};

use crate::chain::EthereumChainInner;
use crate::types::{EvmEvent, EvmMessage, EvmTxResponse};

#[async_trait]
impl MessageSender for EthereumChainInner {
    async fn send_messages(&self, messages: Vec<EvmMessage>) -> Result<Vec<EvmTxResponse>> {
        let mut responses = Vec::with_capacity(messages.len());

        for (idx, msg) in messages.iter().enumerate() {
            let tx = TransactionRequest::default()
                .to(msg.to)
                .input(msg.calldata.clone().into())
                .value(msg.value);

            info!(idx, to = %msg.to, "sending transaction {}/{}", idx + 1, messages.len());

            let pending = self
                .provider
                .send_transaction(tx.clone())
                .await
                .wrap_err("sending transaction")?;

            let tx_hash = *pending.tx_hash();
            info!(%tx_hash, "transaction sent, waiting for receipt");

            let receipt = pending
                .get_receipt()
                .await
                .wrap_err("waiting for transaction receipt")?;

            if !receipt.status() {
                let block = receipt.block_number.unwrap_or(0);
                // Try to replay the call to get the revert reason
                let revert_reason = match self.provider.call(tx).block(block.into()).await {
                    Err(e) => format!("{e}"),
                    Ok(output) => format!("call succeeded unexpectedly: {output}"),
                };
                warn!(%tx_hash, block, gas_used = receipt.gas_used, %revert_reason, "transaction reverted");
                eyre::bail!("transaction {tx_hash} reverted (block {block}): {revert_reason}");
            }

            info!(%tx_hash, gas_used = receipt.gas_used, "transaction confirmed");

            let logs = receipt
                .inner
                .logs()
                .iter()
                .map(EvmEvent::from_alloy_log)
                .collect();

            responses.push(EvmTxResponse {
                tx_hash,
                block_number: receipt.block_number.unwrap_or(0),
                gas_used: receipt.gas_used,
                logs,
            });
        }

        Ok(responses)
    }
}
