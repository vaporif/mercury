use async_trait::async_trait;
use prost::Message;
use sha2::{Digest, Sha256};
use tracing::{debug, instrument};

use mercury_chain_traits::tx::{
    CanEstimateFee, CanPollTxResponse, CanQueryNonce, CanSubmitTx, HasTxTypes,
};
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

impl<S: CosmosSigner> HasTxTypes for CosmosChain<S> {
    type Signer = S;
    type Nonce = CosmosNonce;
    type Fee = CosmosFee;
    type TxHash = String;
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

    let sign_doc_bytes = sign_doc.encode_to_vec();
    let hash = Sha256::digest(&sign_doc_bytes);
    let sig_bytes = signer.sign(hash.into()).await?;

    let tx_raw = TxRaw {
        body_bytes,
        auth_info_bytes,
        signatures: vec![sig_bytes],
    };

    Ok(tx_raw.encode_to_vec())
}

#[async_trait]
impl<S: CosmosSigner> CanQueryNonce for CosmosChain<S> {
    #[instrument(skip_all, name = "query_nonce")]
    async fn query_nonce(&self, signer: &Self::Signer) -> Result<Self::Nonce> {
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
}

impl<S: CosmosSigner> CosmosChain<S> {
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
}

#[async_trait]
impl<S: CosmosSigner> CanEstimateFee for CosmosChain<S> {
    async fn estimate_fee(
        &self,
        signer: &Self::Signer,
        messages: &[Self::Message],
    ) -> Result<Self::Fee> {
        let nonce = self.query_nonce(signer).await?;
        self.estimate_fee_with_nonce(signer, &nonce, messages).await
    }
}

#[async_trait]
impl<S: CosmosSigner> CanSubmitTx for CosmosChain<S> {
    #[instrument(skip_all, name = "submit_tx", fields(seq = nonce.sequence, gas = fee.gas_limit))]
    async fn submit_tx(
        &self,
        signer: &Self::Signer,
        nonce: &Self::Nonce,
        fee: &Self::Fee,
        messages: Vec<Self::Message>,
    ) -> Result<Self::TxHash> {
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
}

#[async_trait]
impl<S: CosmosSigner> CanPollTxResponse for CosmosChain<S> {
    type TxResponse = crate::types::CosmosTxResponse;

    #[instrument(skip_all, name = "poll_tx_response", fields(tx_hash = %tx_hash))]
    async fn poll_tx_response(&self, tx_hash: &Self::TxHash) -> Result<Self::TxResponse> {
        use tendermint::Hash;
        use tendermint_rpc::Client;

        let hash = Hash::from_bytes(tendermint::hash::Algorithm::Sha256, &hex::decode(tx_hash)?)?;

        let max_retries = 10;
        let poll_interval = self.block_time / 2;

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
                        hash: tx_hash.clone(),
                        height: response.height,
                        events,
                    });
                }
                Err(e) => {
                    if attempt == max_retries {
                        eyre::bail!(
                            "transaction {tx_hash} not found after {max_retries} attempts: {e}"
                        );
                    }
                    debug!(
                        attempt = attempt,
                        error = %e,
                        "transaction not yet found, retrying"
                    );
                }
            }
        }

        unreachable!()
    }
}
