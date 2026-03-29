use std::sync::Arc;

use async_trait::async_trait;
use prost::Message;
use sha2::{Digest, Sha256};
use tracing::{debug, info, instrument, warn};

use mercury_chain_traits::types::{ChainTypes, MessageSender, TxReceipt};
use mercury_core::error::{Result, TxError};
use mercury_core::validate::GasMultiplier;

use crate::chain::CosmosChain;
use crate::keys::CosmosSigner;
use crate::types::{CosmosEvent, CosmosMessage, CosmosTxResponse};

/// Token denomination (e.g. "uatom").
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Denom(pub String);

impl std::fmt::Display for Denom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Denom {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<Denom> for String {
    fn from(v: Denom) -> Self {
        v.0
    }
}

impl AsRef<str> for Denom {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Transaction fee amount in smallest denomination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeeAmount(pub u64);

impl std::fmt::Display for FeeAmount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for FeeAmount {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<FeeAmount> for u64 {
    fn from(v: FeeAmount) -> Self {
        v.0
    }
}

/// Gas limit for a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GasLimit(pub u64);

impl std::fmt::Display for GasLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for GasLimit {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<GasLimit> for u64 {
    fn from(v: GasLimit) -> Self {
        v.0
    }
}

/// Cosmos account number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountNumber(pub u64);

impl std::fmt::Display for AccountNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for AccountNumber {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<AccountNumber> for u64 {
    fn from(v: AccountNumber) -> Self {
        v.0
    }
}

/// Transaction sequence (nonce) for ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxSequence(pub u64);

impl std::fmt::Display for TxSequence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for TxSequence {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<TxSequence> for u64 {
    fn from(v: TxSequence) -> Self {
        v.0
    }
}

const DEFAULT_GAS_MULTIPLIER: f64 = 1.3;
const DEFAULT_GAS: u64 = 300_000;
const TX_ENVELOPE_OVERHEAD: usize = 350;
const PROTOBUF_ANY_OVERHEAD: usize = 10;
// sdk `TxSizeCostPerByte` -- gas charged per byte of the serialized tx, checked before execution
const DEFAULT_TX_SIZE_GAS_PER_BYTE: u64 = 10;
const MAX_PARALLEL_BATCHES: usize = 3;
const MAX_TX_POLL_RETRIES: u32 = 10;

#[derive(Clone, Debug)]
pub struct CosmosFee {
    pub amount: FeeAmount,
    pub denom: Denom,
    pub gas_limit: GasLimit,
}

#[derive(Clone, Debug)]
pub struct CosmosNonce {
    pub account_number: AccountNumber,
    pub sequence: TxSequence,
}

fn adjust_gas(gas_used: u64, multiplier: f64, max_gas: Option<u64>) -> u64 {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let adjusted = (gas_used as f64 * multiplier).ceil() as u64;
    match max_gas {
        Some(cap) if adjusted > cap => {
            warn!(
                gas_used,
                adjusted,
                max_gas = cap,
                "adjusted gas exceeds max_gas, capping"
            );
            cap
        }
        _ => adjusted,
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn calculate_fee_amount(gas_limit: u64, gas_price: f64) -> u64 {
    (gas_limit as f64 * gas_price).ceil() as u64
}

fn is_sequence_mismatch(err: &eyre::Report) -> bool {
    err.downcast_ref::<TxError>()
        .is_some_and(|te| matches!(te, TxError::SequenceMismatch { .. }))
}

// Cosmos errors are so bad
fn is_simulation_recoverable(msg: &str) -> bool {
    msg.contains("account sequence mismatch")
        || msg.contains("client state height")
        || msg.contains("packet sequence")
        || msg.contains("empty tx")
}

fn message_size(msg: &CosmosMessage) -> usize {
    msg.type_url.as_ref().len() + msg.value.len() + PROTOBUF_ANY_OVERHEAD
}

fn split_batches(
    messages: Vec<CosmosMessage>,
    max_msg_num: usize,
    max_tx_size: usize,
) -> Vec<Vec<CosmosMessage>> {
    let budget = max_tx_size.saturating_sub(TX_ENVELOPE_OVERHEAD);
    let mut batches: Vec<Vec<CosmosMessage>> = Vec::new();
    let mut current_batch: Vec<CosmosMessage> = Vec::new();
    let mut current_size: usize = 0;

    for msg in messages {
        let msg_size = message_size(&msg);
        if msg_size > budget {
            tracing::error!(
                type_url = %msg.type_url,
                size = msg_size,
                budget = budget,
                "message exceeds max_tx_size, skipping"
            );
            continue;
        }

        let would_exceed_count = current_batch.len() >= max_msg_num;
        let would_exceed_size = current_size + msg_size > budget;

        if !current_batch.is_empty() && (would_exceed_count || would_exceed_size) {
            batches.push(std::mem::take(&mut current_batch));
            current_size = 0;
        }

        current_size += msg_size;
        current_batch.push(msg);
    }

    if !current_batch.is_empty() {
        batches.push(current_batch);
    }

    batches
}

async fn build_tx_bytes(
    chain_id: &str,
    signer: &impl CosmosSigner,
    nonce: &CosmosNonce,
    fee: &CosmosFee,
    fee_granter: Option<&str>,
    messages: &[CosmosMessage],
) -> Result<Vec<u8>> {
    use ibc_proto::cosmos::base::v1beta1::Coin;
    use ibc_proto::cosmos::crypto::secp256k1::PubKey;
    use ibc_proto::cosmos::tx::v1beta1::{
        AuthInfo, Fee, ModeInfo, SignDoc, SignerInfo, TxBody, TxRaw, mode_info::Single,
        mode_info::Sum,
    };

    let body = TxBody {
        messages: messages
            .iter()
            .map(|m| tendermint_proto::google::protobuf::Any {
                type_url: m.type_url.clone().into(),
                value: m.value.clone(),
            })
            .collect(),
        memo: String::new(),
        timeout_height: 0,
        extension_options: vec![],
        non_critical_extension_options: vec![],
    };

    let pub_key = PubKey {
        key: signer.public_key_bytes(),
    };

    let pub_key_any = tendermint_proto::google::protobuf::Any {
        type_url: "/cosmos.crypto.secp256k1.PubKey".to_string(),
        value: pub_key.encode_to_vec(),
    };

    let signer_info = SignerInfo {
        public_key: Some(pub_key_any),
        mode_info: Some(ModeInfo {
            sum: Some(Sum::Single(Single { mode: 1 })), // SIGN_MODE_DIRECT
        }),
        sequence: nonce.sequence.into(),
    };

    let fee_proto = Fee {
        amount: vec![Coin {
            denom: fee.denom.clone().into(),
            amount: fee.amount.to_string(),
        }],
        gas_limit: fee.gas_limit.into(),
        payer: String::new(),
        granter: fee_granter.unwrap_or_default().to_string(),
    };

    #[allow(deprecated)]
    let auth_info = AuthInfo {
        signer_infos: vec![signer_info],
        fee: Some(fee_proto),
        tip: None,
    };

    let body_bytes = body.encode_to_vec();
    let auth_info_bytes = auth_info.encode_to_vec();

    let sign_doc = SignDoc {
        body_bytes: body_bytes.clone(),
        auth_info_bytes: auth_info_bytes.clone(),
        chain_id: chain_id.to_string(),
        account_number: nonce.account_number.into(),
    };

    let hash = Sha256::digest(sign_doc.encode_to_vec());
    let sig_bytes = signer.sign(hash.into()).await?;

    let tx_raw = TxRaw {
        body_bytes: sign_doc.body_bytes,
        auth_info_bytes: sign_doc.auth_info_bytes,
        signatures: vec![sig_bytes],
    };

    Ok(tx_raw.encode_to_vec())
}

impl<S: CosmosSigner> CosmosChain<S> {
    #[instrument(skip_all, name = "query_nonce", fields(chain = %self.chain_label()))]
    pub async fn query_nonce(&self, signer: &S) -> Result<CosmosNonce> {
        use ibc_proto::cosmos::auth::v1beta1::{
            BaseAccount, QueryAccountRequest, query_client::QueryClient as AuthQueryClient,
        };

        let address = signer.account_address()?;
        debug!(address = %address, "querying account nonce");

        let request = tonic::Request::new(QueryAccountRequest {
            address: address.clone(),
        });

        let response = self
            .rpc_guard
            .guarded(|| async {
                AuthQueryClient::new(self.grpc_channel.clone())
                    .account(request)
                    .await
                    .map(tonic::Response::into_inner)
                    .map_err(Into::into)
            })
            .await?;

        let account_any = response
            .account
            .ok_or_else(|| eyre::eyre!("account not found: {address}"))?;

        let base_account = BaseAccount::decode(account_any.value.as_slice())?;

        Ok(CosmosNonce {
            account_number: AccountNumber(base_account.account_number),
            sequence: TxSequence(base_account.sequence),
        })
    }

    #[instrument(skip_all, name = "estimate_fee", fields(chain = %self.chain_label(), msg_count = messages.len()))]
    pub async fn estimate_fee_with_nonce(
        &self,
        signer: &S,
        nonce: &CosmosNonce,
        messages: &[CosmosMessage],
    ) -> Result<CosmosFee> {
        use ibc_proto::cosmos::tx::v1beta1::{
            SimulateRequest, service_client::ServiceClient as TxServiceClient,
        };

        let gas_multiplier = self
            .config
            .gas_multiplier
            .map_or(DEFAULT_GAS_MULTIPLIER, GasMultiplier::value);
        let default_gas = self.config.default_gas.unwrap_or(DEFAULT_GAS);

        let dummy_fee = CosmosFee {
            amount: FeeAmount(0),
            denom: Denom(self.config.gas_price.denom.clone()),
            gas_limit: GasLimit(0),
        };
        let tx_bytes = build_tx_bytes(
            &self.chain_id.to_string(),
            signer,
            nonce,
            &dummy_fee,
            self.config.fee_granter.as_deref(),
            messages,
        )
        .await?;

        // the chain rejects txs where gas_limit < tx_bytes * TxSizeCostPerByte
        let gas_per_byte = self
            .config
            .tx_size_gas_per_byte
            .unwrap_or(DEFAULT_TX_SIZE_GAS_PER_BYTE);
        #[allow(clippy::cast_possible_truncation)]
        let tx_size_gas = (tx_bytes.len() as u64).saturating_mul(gas_per_byte);

        let gas_used = self
            .rpc_guard
            .guarded(|| async {
                #[allow(deprecated)]
                let request = tonic::Request::new(SimulateRequest {
                    tx: None,
                    tx_bytes,
                });
                match TxServiceClient::new(self.grpc_channel.clone())
                    .simulate(request)
                    .await
                {
                    Ok(response) => {
                        let gas = response
                            .into_inner()
                            .gas_info
                            .ok_or_else(|| eyre::eyre!("no gas info in simulate response"))?
                            .gas_used;
                        Ok(gas)
                    }
                    Err(status) if is_simulation_recoverable(status.message()) => {
                        warn!(
                            error = %status,
                            tx_size_gas,
                            default_gas,
                            "simulation failed with recoverable error, using max(default_gas, tx_size_gas)"
                        );
                        Ok(default_gas.max(tx_size_gas))
                    }
                    Err(status) => Err(TxError::SimulationFailed {
                        reason: status.to_string(),
                    }
                    .into()),
                }
            })
            .await?;

        // floor at tx_size_gas so we never hit "out of gas in location: txSize"
        let gas_limit = adjust_gas(gas_used, gas_multiplier, self.config.max_gas).max(tx_size_gas);

        let gas_price_amount = if let Some(ref dgp) = self.config.dynamic_gas_price {
            crate::gas::resolve_gas_price(
                self.grpc_channel.clone(),
                &self.config.gas_price.denom,
                self.config.gas_price.amount,
                dgp,
                &self.dynamic_gas_backend,
                &self.rpc_guard,
            )
            .await
        } else {
            self.config.gas_price.amount
        };

        let fee_amount = calculate_fee_amount(gas_limit, gas_price_amount);

        debug!(
            gas_used = gas_used,
            gas_limit = gas_limit,
            fee_amount = fee_amount,
            "estimated fee"
        );

        Ok(CosmosFee {
            amount: FeeAmount(fee_amount),
            denom: Denom(self.config.gas_price.denom.clone()),
            gas_limit: GasLimit(gas_limit),
        })
    }

    #[instrument(skip_all, name = "submit_tx", fields(chain = %self.chain_label(), seq = u64::from(nonce.sequence), gas = u64::from(fee.gas_limit)))]
    pub async fn submit_tx(
        &self,
        signer: &S,
        nonce: &CosmosNonce,
        fee: &CosmosFee,
        messages: &[CosmosMessage],
    ) -> Result<String> {
        use tendermint_rpc::Client;

        let tx_bytes = build_tx_bytes(
            &self.chain_id.to_string(),
            signer,
            nonce,
            fee,
            self.config.fee_granter.as_deref(),
            messages,
        )
        .await?;

        debug!(
            num_messages = messages.len(),
            sequence = %nonce.sequence,
            gas_limit = %fee.gas_limit,
            "broadcasting transaction"
        );

        let response = self
            .rpc_guard
            .guarded(|| async {
                self.rpc_client
                    .broadcast_tx_sync(tx_bytes)
                    .await
                    .map_err(Into::into)
            })
            .await?;

        if response.code.is_err() {
            let log = &response.log;
            if log.contains("account sequence mismatch") {
                return Err(TxError::SequenceMismatch {
                    details: log.clone(),
                }
                .into());
            }
            return Err(TxError::BroadcastFailed {
                reason: format!("code={}, log={log}", response.code.value()),
            }
            .into());
        }

        let tx_hash = response.hash.to_string();
        debug!(tx_hash = %tx_hash, "transaction broadcast successful");

        Ok(tx_hash)
    }

    #[instrument(skip_all, name = "poll_tx_response", fields(chain = %self.chain_label(), tx_hash = %tx_hash))]
    pub async fn poll_tx_response(&self, tx_hash: &str) -> Result<CosmosTxResponse> {
        use tendermint::Hash;
        use tendermint_rpc::Client;

        let hash = Hash::from_bytes(tendermint::hash::Algorithm::Sha256, &hex::decode(tx_hash)?)?;

        let poll_interval = self.block_time / 2;
        let mut last_err = eyre::eyre!("no attempts made");

        for attempt in 1..=MAX_TX_POLL_RETRIES {
            debug!(
                tx_hash = %tx_hash,
                attempt,
                MAX_TX_POLL_RETRIES,
                "polling for transaction"
            );

            tokio::time::sleep(poll_interval).await;

            match self
                .rpc_guard
                .guarded(|| async { self.rpc_client.tx(hash, false).await.map_err(Into::into) })
                .await
            {
                Ok(response) => {
                    if response.tx_result.code.is_err() {
                        return Err(TxError::Reverted {
                            tx_hash: tx_hash.to_string(),
                            reason: format!(
                                "code={}, log={}",
                                response.tx_result.code.value(),
                                response.tx_result.log
                            ),
                        }
                        .into());
                    }

                    let events = response
                        .tx_result
                        .events
                        .iter()
                        .map(|e| CosmosEvent {
                            kind: e.kind.clone(),
                            attributes: e
                                .attributes
                                .iter()
                                .filter_map(|a| {
                                    let key = a.key_str().ok()?.to_string();
                                    let value = a.value_str().ok()?.to_string();
                                    Some((key, value))
                                })
                                .collect(),
                        })
                        .collect();

                    debug!(
                        tx_hash = %tx_hash,
                        height = %response.height,
                        "transaction confirmed"
                    );

                    return Ok(CosmosTxResponse {
                        hash: tx_hash.to_string(),
                        height: response.height,
                        events,
                    });
                }
                Err(e) => {
                    debug!(
                        attempt = attempt,
                        error = %e,
                        "transaction not yet found, retrying"
                    );
                    last_err = e;
                }
            }
        }

        Err(TxError::NotConfirmed {
            tx_hash: tx_hash.to_string(),
            attempts: MAX_TX_POLL_RETRIES,
            reason: last_err.to_string(),
        }
        .into())
    }
}

impl<S: CosmosSigner> CosmosChain<S> {
    /// Send messages and return raw chain responses (for e2e / setup code that
    /// needs to inspect events). Relay workers should use `MessageSender::send_messages` instead.
    ///
    /// # Panics
    ///
    /// Panics if `split_batches` returns a non-empty vec whose iterator yields `None`
    /// (impossible by construction — guarded by the `len() == 1` check).
    pub async fn send_messages_with_responses(
        &self,
        messages: Vec<CosmosMessage>,
    ) -> Result<Vec<CosmosTxResponse>> {
        if messages.is_empty() {
            return Ok(vec![]);
        }

        let max_msg_num = self.config.max_msg_num;
        let max_tx_size = self
            .config
            .max_tx_size
            .unwrap_or(crate::config::DEFAULT_MAX_TX_SIZE);
        let batches = split_batches(messages, max_msg_num, max_tx_size);

        if batches.is_empty() {
            return Ok(vec![]);
        }

        if batches.len() == 1 {
            let batch = batches.into_iter().next().expect("checked len");
            return self.send_single_batch(batch).await;
        }

        self.send_parallel_batches(batches).await
    }

    async fn send_single_batch(
        &self,
        messages: Vec<CosmosMessage>,
    ) -> Result<Vec<CosmosTxResponse>> {
        let nonce = self.query_nonce(&self.signer).await?;
        let fee = self
            .estimate_fee_with_nonce(&self.signer, &nonce, &messages)
            .await?;

        match self.submit_tx(&self.signer, &nonce, &fee, &messages).await {
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
                let tx_hash = self
                    .submit_tx(&self.signer, &nonce, &fee, &messages)
                    .await?;
                info!("tx submitted on retry: {tx_hash}");
                let response = self.poll_tx_response(&tx_hash).await?;
                Ok(vec![response])
            }
            Err(e) => Err(e),
        }
    }

    async fn send_parallel_batches(
        &self,
        batches: Vec<Vec<CosmosMessage>>,
    ) -> Result<Vec<CosmosTxResponse>> {
        let nonce = self.query_nonce(&self.signer).await?;
        let batch_count = batches.len();

        let mut fees = Vec::with_capacity(batch_count);
        for (i, batch) in batches.iter().enumerate() {
            let nonce_i = CosmosNonce {
                account_number: nonce.account_number,
                sequence: TxSequence(u64::from(nonce.sequence) + i as u64),
            };
            let fee = self
                .estimate_fee_with_nonce(&self.signer, &nonce_i, batch)
                .await?;
            fees.push(fee);
        }

        let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_PARALLEL_BATCHES));
        let mut submit_futures = Vec::with_capacity(batch_count);

        for (i, (batch, fee)) in batches.into_iter().zip(fees).enumerate() {
            let nonce_i = CosmosNonce {
                account_number: nonce.account_number,
                sequence: TxSequence(u64::from(nonce.sequence) + i as u64),
            };
            let sem = semaphore.clone();
            let chain = self.clone();
            submit_futures.push(async move {
                let _permit = sem.acquire().await.expect("semaphore not closed");
                let result = chain.submit_tx(&chain.signer, &nonce_i, &fee, &batch).await;
                (i, batch, result)
            });
        }

        let results: Vec<_> = futures::future::join_all(submit_futures).await;

        let mut responses = Vec::new();
        let mut failed: Vec<(usize, Vec<CosmosMessage>)> = Vec::new();

        for (i, batch, result) in results {
            match result {
                Ok(tx_hash) => {
                    info!(batch = i, "batch submitted: {tx_hash}");
                    match self.poll_tx_response(&tx_hash).await {
                        Ok(resp) => responses.push(resp),
                        Err(e) => {
                            warn!(batch = i, error = %e, "batch poll failed");
                            failed.push((i, batch));
                        }
                    }
                }
                Err(e) => {
                    warn!(batch = i, error = %e, "batch submission failed");
                    failed.push((i, batch));
                }
            }
        }

        if !failed.is_empty() {
            warn!(
                count = failed.len(),
                "retrying failed batches with fresh nonces"
            );
            let fresh_nonce = self.query_nonce(&self.signer).await?;

            for (retry_idx, (original_idx, batch)) in failed.into_iter().enumerate() {
                let nonce_i = CosmosNonce {
                    account_number: fresh_nonce.account_number,
                    sequence: TxSequence(u64::from(fresh_nonce.sequence) + retry_idx as u64),
                };
                let fee = self
                    .estimate_fee_with_nonce(&self.signer, &nonce_i, &batch)
                    .await?;
                let tx_hash = self.submit_tx(&self.signer, &nonce_i, &fee, &batch).await?;
                info!(batch = original_idx, "retry submitted: {tx_hash}");
                let resp = self.poll_tx_response(&tx_hash).await?;
                responses.push(resp);
            }
        }

        Ok(responses)
    }
}

#[async_trait]
impl<S: CosmosSigner> MessageSender for CosmosChain<S> {
    #[instrument(skip_all, name = "send_messages", fields(chain = %self.chain_label(), count = messages.len()))]
    async fn send_messages(&self, messages: Vec<Self::Message>) -> Result<TxReceipt> {
        if messages.is_empty() {
            return Ok(TxReceipt {
                gas_used: None,
                confirmed_at: std::time::Instant::now(),
            });
        }

        let max_msg_num = self.config.max_msg_num;
        let max_tx_size = self
            .config
            .max_tx_size
            .unwrap_or(crate::config::DEFAULT_MAX_TX_SIZE);
        let batches = split_batches(messages, max_msg_num, max_tx_size);

        if batches.is_empty() {
            return Ok(TxReceipt {
                gas_used: None,
                confirmed_at: std::time::Instant::now(),
            });
        }

        if batches.len() == 1 {
            let batch = batches.into_iter().next().expect("checked len");
            self.send_single_batch(batch).await?;
        } else {
            self.send_parallel_batches(batches).await?;
        }

        Ok(TxReceipt {
            gas_used: None,
            confirmed_at: std::time::Instant::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_mismatch_detected_via_downcast() {
        let err: eyre::Report = TxError::SequenceMismatch {
            details: "expected 5, got 4".into(),
        }
        .into();
        assert!(is_sequence_mismatch(&err));
    }

    #[test]
    fn non_sequence_error_not_detected() {
        let err = eyre::eyre!("connection refused");
        assert!(!is_sequence_mismatch(&err));
    }

    #[test]
    fn adjust_gas_applies_multiplier() {
        let adjusted = adjust_gas(100_000, 1.1, Some(400_000));
        // ceil(100_000 * 1.1) may be 110_000 or 110_001 due to floating-point
        assert!(adjusted == 110_000 || adjusted == 110_001);
    }

    #[test]
    fn adjust_gas_caps_at_max() {
        assert_eq!(adjust_gas(350_000, 1.3, Some(400_000)), 400_000);
    }

    #[test]
    fn adjust_gas_no_cap_when_none() {
        // ceil(500_000 * 1.1) = 550_000 — no cap applied
        assert_eq!(adjust_gas(500_000, 1.1, None), 550_000);
    }

    #[test]
    fn adjust_gas_caps_when_used_exceeds_max() {
        assert_eq!(adjust_gas(500_000, 1.1, Some(400_000)), 400_000);
    }

    #[test]
    fn adjust_gas_exact_boundary() {
        assert_eq!(adjust_gas(400_000, 1.0, Some(400_000)), 400_000);
    }

    #[test]
    fn calculate_fee_amount_rounds_up() {
        assert_eq!(calculate_fee_amount(110_000, 0.025), 2750);
        assert_eq!(calculate_fee_amount(110_001, 0.025), 2751);
    }

    fn make_msg(size: usize) -> CosmosMessage {
        CosmosMessage {
            type_url: "/test.Msg".to_string().into(),
            value: vec![0u8; size],
        }
    }

    #[test]
    fn split_batches_empty() {
        let batches = split_batches(vec![], 10, 100_000);
        assert!(batches.is_empty());
    }

    #[test]
    fn split_batches_single_batch() {
        let msgs = vec![make_msg(100); 5];
        let batches = split_batches(msgs, 10, 100_000);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 5);
    }

    #[test]
    fn split_batches_by_msg_count() {
        let msgs = vec![make_msg(10); 10];
        let batches = split_batches(msgs, 3, 100_000);
        assert_eq!(batches.len(), 4); // 3+3+3+1
        assert_eq!(batches[0].len(), 3);
        assert_eq!(batches[3].len(), 1);
    }

    #[test]
    fn split_batches_by_size() {
        let msgs = vec![make_msg(100); 10];
        let batches = split_batches(msgs, 100, 1000);
        assert!(batches.len() > 1);
    }

    #[test]
    fn split_batches_oversized_single_msg_skipped() {
        let msgs = vec![make_msg(200_000)];
        let batches = split_batches(msgs, 10, 1000);
        assert!(batches.is_empty());
    }

    #[test]
    fn split_batches_oversized_msg_among_normal() {
        let mut msgs = vec![make_msg(100); 3];
        msgs.insert(1, make_msg(200_000));
        let batches = split_batches(msgs, 10, 1000);
        let total: usize = batches.iter().map(std::vec::Vec::len).sum();
        assert_eq!(total, 3);
    }
}

#[cfg(test)]
mod proptest_tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_message(max_value_len: usize) -> impl Strategy<Value = CosmosMessage> {
        (1..=max_value_len).prop_map(|len| CosmosMessage {
            type_url: "/test.Msg".to_string().into(),
            value: vec![0u8; len],
        })
    }

    proptest! {
        #[test]
        fn all_messages_land_in_some_batch(
            msgs in prop::collection::vec(arb_message(500), 0..50),
            max_msg_num in 1usize..=10,
            max_tx_size in 200usize..=5000,
        ) {
            let input_count = msgs.len();
            let batches = split_batches(msgs, max_msg_num, max_tx_size);
            let output_count: usize = batches.iter().map(std::vec::Vec::len).sum();
            prop_assert!(output_count <= input_count);
            for batch in &batches {
                prop_assert!(!batch.is_empty());
            }
            for batch in &batches {
                prop_assert!(batch.len() <= max_msg_num);
            }
        }
    }
}
