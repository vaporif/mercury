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

    /// Send a single EVM message: estimate gas, broadcast, wait for receipt,
    /// and return the response. Shared by both `send_messages_with_responses`
    /// and `MessageSender::send_messages`.
    async fn send_single_message(
        &self,
        msg: &EvmMessage,
        idx: usize,
        total: usize,
    ) -> Result<EvmTxResponse> {
        let mut tx = TransactionRequest::default()
            .to(msg.to)
            .input(msg.calldata.clone().into())
            .value(msg.value);

        self.estimate_and_set_gas(&mut tx).await?;

        info!(idx, to = %msg.to, "sending transaction {}/{}", idx + 1, total);

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
                Err(e) => format!("{e:#}"),
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
            .collect::<eyre::Result<Vec<_>>>()?;

        Ok(EvmTxResponse {
            tx_hash,
            block_number: BlockNumber(receipt.block_number.unwrap_or(0)),
            gas_used: GasUsed(receipt.gas_used),
            logs,
        })
    }

    /// Send messages and return raw chain responses (for CLI commands and setup
    /// code that need to inspect events). Relay workers should use
    /// `MessageSender::send_messages` instead.
    pub async fn send_messages_with_responses(
        &self,
        messages: Vec<EvmMessage>,
    ) -> Result<Vec<EvmTxResponse>> {
        let total = messages.len();
        let mut responses = Vec::with_capacity(total);
        for (idx, msg) in messages.iter().enumerate() {
            responses.push(self.send_single_message(msg, idx, total).await?);
        }
        Ok(responses)
    }
}

#[async_trait]
impl MessageSender for EthereumChain {
    // TODO: use ICS26Router::multicall to batch all messages into a single tx
    async fn send_messages(&self, messages: Vec<EvmMessage>) -> Result<TxReceipt> {
        let total = messages.len();
        let mut total_gas: u64 = 0;
        for (idx, msg) in messages.iter().enumerate() {
            let response = self.send_single_message(msg, idx, total).await?;
            total_gas += response.gas_used.0;
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
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "values stay well below 2^53"
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
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "values stay well below 2^53"
)]
fn mul_ceil_u128(base: u128, multiplier: f64) -> u128 {
    let result = (base as f64 * multiplier).ceil();
    if result >= u128::MAX as f64 {
        u128::MAX
    } else {
        result.max(0.0) as u128
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use alloy::primitives::{Address, B256, U256};
    use mercury_core::validate::GasMultiplier;
    use serde_json::{Value, json};

    use super::*;
    use crate::chain::test_support::{ROUTER, chain_at, no_rpc, spawn_rpc};

    const TX_HASH: B256 = B256::repeat_byte(0x99);

    fn msg(calldata: &[u8]) -> EvmMessage {
        EvmMessage {
            to: ROUTER,
            calldata: calldata.to_vec(),
            value: U256::ZERO,
        }
    }

    fn receipt_json(success: bool, gas_used: u64, logs: &Value) -> Value {
        json!({
            "transactionHash": TX_HASH,
            "transactionIndex": "0x0",
            "blockHash": B256::repeat_byte(0x01),
            "blockNumber": "0x10",
            "from": Address::repeat_byte(0x03),
            "to": ROUTER,
            "cumulativeGasUsed": format!("{gas_used:#x}"),
            "gasUsed": format!("{gas_used:#x}"),
            "effectiveGasPrice": "0x1",
            "contractAddress": null,
            "logs": logs,
            "logsBloom": format!("0x{}", "00".repeat(256)),
            "type": "0x2",
            "status": if success { "0x1" } else { "0x0" },
        })
    }

    fn log_json(log_index: Option<u64>) -> Value {
        json!({
            "address": ROUTER,
            "topics": [B256::repeat_byte(0x44)],
            "data": "0x0102",
            "blockNumber": "0x10",
            "blockHash": B256::repeat_byte(0x01),
            "transactionHash": TX_HASH,
            "transactionIndex": "0x0",
            "logIndex": log_index.map(|i| format!("{i:#x}")),
            "removed": false,
        })
    }

    /// Records every RPC method called and serves a successful-or-reverted tx.
    async fn tx_server(success: bool, logs: Value) -> (String, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        let url = spawn_rpc(move |method, _| {
            recorded.lock().unwrap().push(method.to_string());
            match method {
                "eth_sendTransaction" => Ok(json!(TX_HASH)),
                "eth_getTransactionReceipt" => Ok(receipt_json(success, 21_000, &logs)),
                "eth_blockNumber" => Ok(json!("0x10")),
                "eth_call" => Err("execution reverted: IBCInvalidProof".to_string()),
                "eth_estimateGas" => Ok(json!("0x5208")),
                _ => Ok(Value::Null),
            }
        })
        .await;
        (url, calls)
    }

    #[test]
    fn mul_ceil_u64_rounds_up() {
        assert_eq!(mul_ceil_u64(100, 1.5), 150);
        assert_eq!(mul_ceil_u64(101, 1.1), 112);
        assert_eq!(mul_ceil_u64(21_000, 1.0), 21_000);
        assert_eq!(mul_ceil_u64(0, 3.0), 0);
    }

    #[test]
    fn mul_ceil_u64_saturates_and_clamps() {
        assert_eq!(mul_ceil_u64(u64::MAX, 2.0), u64::MAX);
        assert_eq!(mul_ceil_u64(10, -1.0), 0);
    }

    #[test]
    fn mul_ceil_u128_rounds_up_and_saturates() {
        assert_eq!(mul_ceil_u128(1_000_000_001, 1.5), 1_500_000_002);
        assert_eq!(mul_ceil_u128(u128::MAX, 2.0), u128::MAX);
        assert_eq!(mul_ceil_u128(5, -2.0), 0);
    }

    #[tokio::test]
    async fn gas_untouched_without_gas_config() {
        let url = spawn_rpc(no_rpc).await;
        let chain = chain_at(&url);
        let mut tx = TransactionRequest::default();
        chain.estimate_and_set_gas(&mut tx).await.unwrap();
        assert_eq!(tx.gas, None);
        assert_eq!(tx.max_priority_fee_per_gas, None);
    }

    #[tokio::test]
    async fn gas_limit_multiplied_and_capped() {
        let url = spawn_rpc(|method, _| match method {
            "eth_estimateGas" => Ok(json!("0x64")), // 100
            other => Err(format!("unexpected {other}")),
        })
        .await;
        let mut chain = chain_at(&url);
        chain.config.gas_multiplier = Some(GasMultiplier::new_unchecked(1.5));

        let mut tx = TransactionRequest::default();
        chain.estimate_and_set_gas(&mut tx).await.unwrap();
        assert_eq!(tx.gas, Some(150));
        assert_eq!(tx.max_priority_fee_per_gas, None);

        chain.config.max_gas = Some(120);
        let mut tx = TransactionRequest::default();
        chain.estimate_and_set_gas(&mut tx).await.unwrap();
        assert_eq!(tx.gas, Some(120));
    }

    #[tokio::test]
    async fn max_gas_alone_caps_raw_estimate() {
        let url = spawn_rpc(|_, _| Ok(json!("0x3e8"))).await; // 1000
        let mut chain = chain_at(&url);
        chain.config.max_gas = Some(500);
        let mut tx = TransactionRequest::default();
        chain.estimate_and_set_gas(&mut tx).await.unwrap();
        assert_eq!(tx.gas, Some(500));
    }

    #[tokio::test]
    async fn priority_fee_multiplier_applied() {
        let url = spawn_rpc(|method, _| match method {
            "eth_estimateGas" => Ok(json!("0x64")),
            "eth_maxPriorityFeePerGas" => Ok(json!("0x3e8")), // 1000
            other => Err(format!("unexpected {other}")),
        })
        .await;
        let mut chain = chain_at(&url);
        chain.config.max_priority_fee_multiplier = Some(GasMultiplier::new_unchecked(1.25));
        let mut tx = TransactionRequest::default();
        chain.estimate_and_set_gas(&mut tx).await.unwrap();
        assert_eq!(tx.gas, Some(100));
        assert_eq!(tx.max_priority_fee_per_gas, Some(1250));
    }

    #[tokio::test]
    async fn estimate_failure_propagates() {
        let url = spawn_rpc(|_, _| Err("out of gas".to_string())).await;
        let mut chain = chain_at(&url);
        chain.config.max_gas = Some(1);
        let mut tx = TransactionRequest::default();
        assert!(chain.estimate_and_set_gas(&mut tx).await.is_err());
    }

    #[tokio::test]
    async fn send_messages_with_responses_returns_receipt_data() {
        let (url, calls) = tx_server(true, json!([log_json(Some(2))])).await;
        let chain = chain_at(&url);
        let responses = chain
            .send_messages_with_responses(vec![msg(&[1]), msg(&[2])])
            .await
            .unwrap();
        assert_eq!(responses.len(), 2);
        let r = &responses[0];
        assert_eq!(r.tx_hash, TX_HASH);
        assert_eq!(r.block_number, BlockNumber(16));
        assert_eq!(r.gas_used, GasUsed(21_000));
        assert_eq!(r.logs.len(), 1);
        assert_eq!(r.logs[0].data, vec![1, 2]);
        let calls = calls.lock().unwrap().clone();
        assert_eq!(
            calls.iter().filter(|m| *m == "eth_sendTransaction").count(),
            2
        );
        assert!(!calls.iter().any(|m| m == "eth_estimateGas"));
    }

    #[tokio::test]
    async fn send_messages_sums_gas() {
        let (url, _) = tx_server(true, json!([])).await;
        let receipt = chain_at(&url)
            .send_messages(vec![msg(&[1]), msg(&[2]), msg(&[3])])
            .await
            .unwrap();
        assert_eq!(receipt.gas_used, Some(63_000));
    }

    #[tokio::test]
    async fn send_messages_empty_batch_is_noop() {
        let url = spawn_rpc(no_rpc).await;
        let receipt = chain_at(&url).send_messages(vec![]).await.unwrap();
        assert_eq!(receipt.gas_used, Some(0));
    }

    #[tokio::test]
    async fn reverted_tx_reports_simulated_reason() {
        let (url, calls) = tx_server(false, json!([])).await;
        let err = chain_at(&url)
            .send_messages_with_responses(vec![msg(&[1]), msg(&[2])])
            .await
            .unwrap_err();
        match err.downcast_ref::<TxError>() {
            Some(TxError::Reverted { tx_hash, reason }) => {
                assert_eq!(tx_hash, &TX_HASH.to_string());
                assert!(reason.starts_with("block 16:"), "{reason}");
                assert!(
                    reason.contains("simulating reverted transaction"),
                    "{reason}"
                );
                assert!(reason.contains("IBCInvalidProof"), "{reason}");
            }
            other => panic!("expected Reverted, got {other:?}"),
        }
        // Stops at the first failure.
        let calls = calls.lock().unwrap().clone();
        assert_eq!(
            calls.iter().filter(|m| *m == "eth_sendTransaction").count(),
            1
        );
    }

    #[tokio::test]
    async fn receipt_with_malformed_log_errors() {
        let (url, _) = tx_server(true, json!([log_json(None)])).await;
        let err = chain_at(&url)
            .send_messages_with_responses(vec![msg(&[1])])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("log_index"), "{err}");
    }

    #[tokio::test]
    async fn send_failure_propagates() {
        let url = spawn_rpc(|_, _| Err("insufficient funds".to_string())).await;
        let err = chain_at(&url)
            .send_messages(vec![msg(&[1])])
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("insufficient funds"), "{err:#}");
    }
}
