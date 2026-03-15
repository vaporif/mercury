use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use async_trait::async_trait;
use eyre::Context;
use mercury_chain_traits::types::MessageSender;
use mercury_core::error::Result;
use tracing::info;

use crate::chain::EthereumChainInner;
use crate::types::{EvmEvent, EvmMessage, EvmTxResponse};

#[async_trait]
impl MessageSender for EthereumChainInner {
    async fn send_messages(&self, messages: Vec<EvmMessage>) -> Result<Vec<EvmTxResponse>> {
        let mut responses = Vec::with_capacity(messages.len());

        for msg in messages {
            let tx = TransactionRequest::default()
                .to(msg.to)
                .input(msg.calldata.into())
                .value(msg.value);

            let pending = self
                .provider
                .send_transaction(tx)
                .await
                .wrap_err("sending transaction")?;

            let tx_hash = *pending.tx_hash();
            info!(%tx_hash, "transaction sent, waiting for receipt");

            let receipt = pending
                .get_receipt()
                .await
                .wrap_err("waiting for transaction receipt")?;

            if !receipt.status() {
                eyre::bail!(
                    "transaction {tx_hash} reverted (block {})",
                    receipt.block_number.unwrap_or(0)
                );
            }

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
