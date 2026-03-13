use std::fmt::{Debug, Display};

use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::HasChainTypes;

/// Associated types for transaction submission (signer, nonce, fee, tx hash).
pub trait HasTxTypes: HasChainTypes {
    type Signer: ThreadSafe;
    type Nonce: Clone + ThreadSafe;
    type Fee: Clone + ThreadSafe;
    type TxHash: Clone + Debug + Display + ThreadSafe;
}

/// Submits a signed transaction with messages to the chain.
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

/// Estimates the fee for a set of messages.
#[async_trait]
pub trait CanEstimateFee: HasTxTypes {
    async fn estimate_fee(
        &self,
        signer: &Self::Signer,
        messages: &[Self::Message],
    ) -> Result<Self::Fee>;
}

/// Queries the current nonce (sequence number) for a signer.
#[async_trait]
pub trait CanQueryNonce: HasTxTypes {
    async fn query_nonce(&self, signer: &Self::Signer) -> Result<Self::Nonce>;
}

/// Polls for a transaction response by its hash.
#[async_trait]
pub trait CanPollTxResponse: HasTxTypes {
    type TxResponse: ThreadSafe;
    async fn poll_tx_response(&self, tx_hash: &Self::TxHash) -> Result<Self::TxResponse>;
}
