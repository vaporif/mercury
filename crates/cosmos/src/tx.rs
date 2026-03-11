use async_trait::async_trait;

use mercury_chain_traits::tx::{
    CanEstimateFee, CanPollTxResponse, CanQueryNonce, CanSubmitTx, HasTxTypes,
};
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::keys::Secp256k1KeyPair;

#[derive(Clone, Debug)]
pub struct CosmosFee {
    pub amount: u64,
    pub denom: String,
    pub gas_limit: u64,
}

#[derive(Clone, Debug)]
pub struct CosmosNonce {
    pub account_number: u64,
    pub sequence: u64,
}

impl HasTxTypes for CosmosChain {
    type Signer = Secp256k1KeyPair;
    type Nonce = CosmosNonce;
    type Fee = CosmosFee;
    type TxHash = String;
}

#[async_trait]
impl CanSubmitTx for CosmosChain {
    async fn submit_tx(
        &self,
        _signer: &Self::Signer,
        _nonce: &Self::Nonce,
        _fee: &Self::Fee,
        _messages: Vec<Self::Message>,
    ) -> Result<Self::TxHash> {
        // TODO: encode tx body + auth info, sign, broadcast via tendermint-rpc
        todo!("submit tx")
    }
}

#[async_trait]
impl CanEstimateFee for CosmosChain {
    async fn estimate_fee(
        &self,
        _signer: &Self::Signer,
        _messages: &[Self::Message],
    ) -> Result<Self::Fee> {
        // TODO: simulate tx via gRPC, extract gas_used, apply multiplier
        todo!("estimate fee")
    }
}

#[async_trait]
impl CanQueryNonce for CosmosChain {
    async fn query_nonce(&self, _signer: &Self::Signer) -> Result<Self::Nonce> {
        // TODO: query account via gRPC to get account_number + sequence
        todo!("query nonce")
    }
}

#[async_trait]
impl CanPollTxResponse for CosmosChain {
    type TxResponse = crate::types::CosmosTxResponse;

    async fn poll_tx_response(&self, _tx_hash: &Self::TxHash) -> Result<Self::TxResponse> {
        // TODO: poll tx by hash via tendermint-rpc, with retry loop
        todo!("poll tx response")
    }
}
