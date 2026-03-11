use std::fmt::{Debug, Display};

use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::HasMessageTypes;

pub trait HasTxTypes: HasMessageTypes {
    type Signer: ThreadSafe;
    type Nonce: Clone + ThreadSafe;
    type Fee: Clone + ThreadSafe;
    type TxHash: Clone + Debug + Display + ThreadSafe;
}

#[async_trait]
pub trait CanSubmitTx: HasTxTypes {
    async fn submit_tx(
        &self,
        signer: &Self::Signer,
        nonce: &Self::Nonce,
        fee: &Self::Fee,
        messages: Vec<Self::Message>,
    ) -> Result<Self::TxHash>;
}

#[async_trait]
pub trait CanEstimateFee: HasTxTypes {
    async fn estimate_fee(
        &self,
        signer: &Self::Signer,
        messages: &[Self::Message],
    ) -> Result<Self::Fee>;
}

#[async_trait]
pub trait CanQueryNonce: HasTxTypes {
    async fn query_nonce(&self, signer: &Self::Signer) -> Result<Self::Nonce>;
}

#[async_trait]
pub trait CanPollTxResponse: HasTxTypes {
    type TxResponse: ThreadSafe;
    async fn poll_tx_response(&self, tx_hash: &Self::TxHash) -> Result<Self::TxResponse>;
}
