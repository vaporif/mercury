use alloy::network::TransactionBuilder;
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use async_trait::async_trait;
use eyre::Context;
use mercury_chain_traits::types::{MessageSender, TxReceipt};
use mercury_core::error::{Result, TxError};
use tracing::{debug, info, warn};

use crate::chain::EthereumChain;
use crate::types::{BlockNumber, EvmEvent, EvmMessage, EvmTxResponse, GasUsed};

impl EthereumChain {
    async fn estimate_and_set_gas(&self, tx: &mut TransactionRequest) -> Result<()> {
        if self.config.gas_multiplier.is_none()
            && self.config.max_gas.is_none()
            && self.config.max_priority_fee_multiplier.is_none()
        {
            return Ok(());
        }

        let estimated = self
            .rpc_guard
            .guarded(|| async {
                self.provider
                    .estimate_gas(tx.clone())
                    .await
                    .wrap_err("estimating gas")
            })
            .await?;

        let mut gas_limit = self
            .config
            .gas_multiplier
            .map_or(estimated, |m| mul_ceil_u64(estimated, m.value()));

        if let Some(max) = self.config.max_gas {
            gas_limit = gas_limit.min(max);
        }

        tx.set_gas_limit(gas_limit);

        if let Some(multiplier) = self.config.max_priority_fee_multiplier {
            let priority_fee = self
                .rpc_guard
                .guarded(|| async {
                    self.provider
                        .get_max_priority_fee_per_gas()
                        .await
                        .wrap_err("querying max priority fee per gas")
                })
                .await?;

            let adjusted = mul_ceil_u128(priority_fee, multiplier.value());
            tx.set_max_priority_fee_per_gas(adjusted);
        }

        debug!(estimated, gas_limit, "gas estimated for transaction");
        Ok(())
    }

    /// Send messages and return raw chain responses (for CLI commands and setup
    /// code that need to inspect events). Relay workers should use
    /// `MessageSender::send_messages` instead.
    pub async fn send_messages_with_responses(
        &self,
        messages: Vec<EvmMessage>,
    ) -> Result<Vec<EvmTxResponse>> {
        let mut responses = Vec::with_capacity(messages.len());

        for (idx, msg) in messages.iter().enumerate() {
            let mut tx = TransactionRequest::default()
                .to(msg.to)
                .input(msg.calldata.clone().into())
                .value(msg.value);

            self.estimate_and_set_gas(&mut tx).await?;

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
                block_number: BlockNumber(receipt.block_number.unwrap_or(0)),
                gas_used: GasUsed(receipt.gas_used),
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
            let mut tx = TransactionRequest::default()
                .to(msg.to)
                .input(msg.calldata.clone().into())
                .value(msg.value);

            self.estimate_and_set_gas(&mut tx).await?;

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

/// Multiply `base` by `multiplier`, rounding up, saturating at `u64::MAX`.
///
/// Precision loss on the `u64 → f64` path is acceptable: real gas values
/// are well below 2^53, so the f64 mantissa covers them exactly.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn mul_ceil_u64(base: u64, multiplier: f64) -> u64 {
    let result = (base as f64 * multiplier).ceil();
    if result >= u64::MAX as f64 {
        u64::MAX
    } else {
        result.max(0.0) as u64
    }
}

/// Same as [`mul_ceil_u64`] but for `u128` (used for wei-denominated fees).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn mul_ceil_u128(base: u128, multiplier: f64) -> u128 {
    let result = (base as f64 * multiplier).ceil();
    if result >= u128::MAX as f64 {
        u128::MAX
    } else {
        result.max(0.0) as u128
    }
}
