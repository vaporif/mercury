use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use async_trait::async_trait;
use eyre::Context;
use mercury_chain_traits::types::{MessageSender, TxReceipt};
use mercury_core::error::{Result, TxError};
use tracing::{info, warn};

use crate::chain::EthereumChain;
use crate::types::{EvmEvent, EvmMessage, EvmTxResponse};

impl EthereumChain {
    /// Send messages and return raw chain responses (for e2e / setup code that
    /// needs to inspect events). Relay workers should use `MessageSender::send_messages` instead.
    pub async fn send_messages_with_responses(
        &self,
        messages: Vec<EvmMessage>,
    ) -> Result<Vec<EvmTxResponse>> {
        let mut responses = Vec::with_capacity(messages.len());

        for (idx, msg) in messages.iter().enumerate() {
            let tx = TransactionRequest::default()
                .to(msg.to)
                .input(msg.calldata.clone().into())
                .value(msg.value);

            info!(idx, to = %msg.to, "sending transaction {}/{}", idx + 1, messages.len());

            let pending = self
                .rpc_guard
                .guarded(|| async {
                    self.provider
                        .send_transaction(tx.clone())
                        .await
                        .wrap_err("sending transaction")
                })
                .await?;

            let tx_hash = *pending.tx_hash();
            info!(%tx_hash, "transaction sent, waiting for receipt");

            let receipt = pending
                .get_receipt()
                .await
                .wrap_err("waiting for transaction receipt")?;

            if !receipt.status() {
                let block = receipt.block_number.unwrap_or(0);
                let revert_reason = match self
                    .rpc_guard
                    .guarded(|| async {
                        self.provider
                            .call(tx)
                            .block(block.into())
                            .await
                            .wrap_err("simulating reverted transaction")
                    })
                    .await
                {
                    Err(e) => format!("{e}"),
                    Ok(output) => format!("call succeeded unexpectedly: {output}"),
                };
                warn!(%tx_hash, block, gas_used = receipt.gas_used, %revert_reason, "transaction reverted");
                return Err(TxError::Reverted {
                    tx_hash: tx_hash.to_string(),
                    reason: format!("block {block}: {revert_reason}"),
                }
                .into());
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

#[async_trait]
impl MessageSender for EthereumChain {
    // TODO: use ICS26Router::multicall to batch all messages into a single tx
    async fn send_messages(&self, messages: Vec<EvmMessage>) -> Result<TxReceipt> {
        let mut total_gas: u64 = 0;

        for (idx, msg) in messages.iter().enumerate() {
            let tx = TransactionRequest::default()
                .to(msg.to)
                .input(msg.calldata.clone().into())
                .value(msg.value);

            info!(idx, to = %msg.to, "sending transaction {}/{}", idx + 1, messages.len());

            let pending = self
                .rpc_guard
                .guarded(|| async {
                    self.provider
                        .send_transaction(tx.clone())
                        .await
                        .wrap_err("sending transaction")
                })
                .await?;

            let tx_hash = *pending.tx_hash();
            info!(%tx_hash, "transaction sent, waiting for receipt");

            let receipt = pending
                .get_receipt()
                .await
                .wrap_err("waiting for transaction receipt")?;

            if !receipt.status() {
                let block = receipt.block_number.unwrap_or(0);
                let revert_reason = match self
                    .rpc_guard
                    .guarded(|| async {
                        self.provider
                            .call(tx)
                            .block(block.into())
                            .await
                            .wrap_err("simulating reverted transaction")
                    })
                    .await
                {
                    Err(e) => format!("{e}"),
                    Ok(output) => format!("call succeeded unexpectedly: {output}"),
                };
                warn!(%tx_hash, block, gas_used = receipt.gas_used, %revert_reason, "transaction reverted");
                return Err(TxError::Reverted {
                    tx_hash: tx_hash.to_string(),
                    reason: format!("block {block}: {revert_reason}"),
                }
                .into());
            }

            info!(%tx_hash, gas_used = receipt.gas_used, "transaction confirmed");
            total_gas += receipt.gas_used;
        }

        Ok(TxReceipt {
            gas_used: Some(total_gas),
            confirmed_at: std::time::Instant::now(),
        })
    }
}
