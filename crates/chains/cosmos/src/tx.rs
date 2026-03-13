use async_trait::async_trait;
use prost::Message;
use sha2::{Digest, Sha256};
use tracing::{debug, info, instrument, warn};

use mercury_chain_traits::types::MessageSender;
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::keys::CosmosSigner;
use crate::queries::grpc_unary;

/// Transaction fee with gas limit and token denomination.
#[derive(Clone, Debug)]
pub struct CosmosFee {
    pub amount: u64,
    pub denom: String,
    pub gas_limit: u64,
}

/// Account number and sequence for transaction replay protection.
#[derive(Clone, Debug)]
pub struct CosmosNonce {
    pub account_number: u64,
    pub sequence: u64,
}

async fn build_tx_bytes(
    chain_id: &str,
    signer: &impl CosmosSigner,
    nonce: &CosmosNonce,
    fee: &CosmosFee,
    messages: &[crate::types::CosmosMessage],
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
                type_url: m.type_url.clone(),
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
        sequence: nonce.sequence,
    };

    let fee_proto = Fee {
        amount: vec![Coin {
            denom: fee.denom.clone(),
            amount: fee.amount.to_string(),
        }],
        gas_limit: fee.gas_limit,
        payer: String::new(),
        granter: String::new(),
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
        account_number: nonce.account_number,
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
    #[instrument(skip_all, name = "query_nonce")]
    pub async fn query_nonce(&self, signer: &S) -> Result<CosmosNonce> {
        use ibc_proto::cosmos::auth::v1beta1::{
            BaseAccount, QueryAccountRequest, QueryAccountResponse,
        };

        let address = signer.account_address()?;
        debug!(address = %address, "querying account nonce");

        let request = tonic::Request::new(QueryAccountRequest {
            address: address.clone(),
        });

        let response = grpc_unary::<QueryAccountRequest, QueryAccountResponse>(
            self.grpc_channel.clone(),
            "/cosmos.auth.v1beta1.Query/Account",
            request,
        )
        .await?
        .into_inner();

        let account_any = response
            .account
            .ok_or_else(|| eyre::eyre!("account not found: {address}"))?;

        let base_account = BaseAccount::decode(account_any.value.as_slice())?;

        Ok(CosmosNonce {
            account_number: base_account.account_number,
            sequence: base_account.sequence,
        })
    }

    #[instrument(skip_all, name = "estimate_fee", fields(msg_count = messages.len()))]
    pub async fn estimate_fee_with_nonce(
        &self,
        signer: &S,
        nonce: &CosmosNonce,
        messages: &[crate::types::CosmosMessage],
    ) -> Result<CosmosFee> {
        use ibc_proto::cosmos::tx::v1beta1::{SimulateRequest, SimulateResponse};

        let dummy_fee = CosmosFee {
            amount: 0,
            denom: self.config.gas_price.denom.clone(),
            gas_limit: 0,
        };
        let tx_bytes = build_tx_bytes(
            &self.chain_id.to_string(),
            signer,
            nonce,
            &dummy_fee,
            messages,
        )
        .await?;

        #[allow(deprecated)]
        let request = tonic::Request::new(SimulateRequest { tx: None, tx_bytes });

        let response = grpc_unary::<SimulateRequest, SimulateResponse>(
            self.grpc_channel.clone(),
            "/cosmos.tx.v1beta1.Service/Simulate",
            request,
        )
        .await?
        .into_inner();

        let gas_info = response
            .gas_info
            .ok_or_else(|| eyre::eyre!("no gas info in simulate response"))?;

        let gas_used = gas_info.gas_used;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let gas_limit = (gas_used as f64 * 1.3) as u64;

        let gas_price = &self.config.gas_price;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let fee_amount = (gas_limit as f64 * gas_price.amount).ceil() as u64;

        debug!(
            gas_used = gas_used,
            gas_limit = gas_limit,
            fee_amount = fee_amount,
            "estimated fee"
        );

        Ok(CosmosFee {
            amount: fee_amount,
            denom: gas_price.denom.clone(),
            gas_limit,
        })
    }

    #[instrument(skip_all, name = "submit_tx", fields(seq = nonce.sequence, gas = fee.gas_limit))]
    pub async fn submit_tx(
        &self,
        signer: &S,
        nonce: &CosmosNonce,
        fee: &CosmosFee,
        messages: Vec<crate::types::CosmosMessage>,
    ) -> Result<String> {
        use tendermint_rpc::Client;

        let tx_bytes =
            build_tx_bytes(&self.chain_id.to_string(), signer, nonce, fee, &messages).await?;

        debug!(
            num_messages = messages.len(),
            sequence = nonce.sequence,
            gas_limit = fee.gas_limit,
            "broadcasting transaction"
        );

        let response = self.rpc_client.broadcast_tx_sync(tx_bytes).await?;

        if response.code.is_err() {
            eyre::bail!(
                "broadcast_tx_sync failed: code={}, log={}",
                response.code.value(),
                response.log
            );
        }

        let tx_hash = response.hash.to_string();
        debug!(tx_hash = %tx_hash, "transaction broadcast successful");

        Ok(tx_hash)
    }

    #[instrument(skip_all, name = "poll_tx_response", fields(tx_hash = %tx_hash))]
    pub async fn poll_tx_response(&self, tx_hash: &str) -> Result<crate::types::CosmosTxResponse> {
        use tendermint::Hash;
        use tendermint_rpc::Client;

        let hash = Hash::from_bytes(tendermint::hash::Algorithm::Sha256, &hex::decode(tx_hash)?)?;

        let max_retries = 10;
        let poll_interval = self.block_time / 2;
        let mut last_err = eyre::eyre!("no attempts made");

        for attempt in 1..=max_retries {
            debug!(
                tx_hash = %tx_hash,
                attempt = attempt,
                max_retries = max_retries,
                "polling for transaction"
            );

            tokio::time::sleep(poll_interval).await;

            match self.rpc_client.tx(hash, false).await {
                Ok(response) => {
                    if response.tx_result.code.is_err() {
                        eyre::bail!(
                            "transaction failed: code={}, log={}",
                            response.tx_result.code.value(),
                            response.tx_result.log
                        );
                    }

                    let events = response
                        .tx_result
                        .events
                        .iter()
                        .map(|e| crate::types::CosmosEvent {
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

                    return Ok(crate::types::CosmosTxResponse {
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
                    last_err = e.into();
                }
            }
        }

        eyre::bail!("transaction {tx_hash} not found after {max_retries} attempts: {last_err}")
    }
}

fn is_sequence_mismatch(e: &eyre::Report) -> bool {
    let msg = e.to_string();
    msg.contains("account sequence mismatch")
}

#[async_trait]
impl<S: CosmosSigner> MessageSender for CosmosChain<S> {
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
