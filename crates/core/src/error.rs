//! Error handling — re-exports from `eyre` plus typed domain errors.

pub use eyre::{bail, eyre, Context, Report, Result, WrapErr};

/// Whether an error is safe to retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retryability {
    Retryable,
    Fatal,
}

/// Implemented by domain error types that carry a default retryability.
pub trait HasRetryability {
    fn retryability(&self) -> Retryability;
}

/// Extension trait on `eyre::Report` for retryability checks.
/// Uses `downcast_ref` (which walks the error chain internally)
/// to find typed errors. Untyped errors default to `Retryable`.
pub trait RetryableExt {
    fn retryability(&self) -> Retryability;
    fn is_retryable(&self) -> bool;
}

impl RetryableExt for eyre::Report {
    fn retryability(&self) -> Retryability {
        if let Some(e) = self.downcast_ref::<TxError>() {
            return e.retryability();
        }
        if let Some(e) = self.downcast_ref::<QueryError>() {
            return e.retryability();
        }
        if let Some(e) = self.downcast_ref::<ProofError>() {
            return e.retryability();
        }
        if let Some(e) = self.downcast_ref::<ClientError>() {
            return e.retryability();
        }
        if let Some(e) = self.downcast_ref::<RpcError>() {
            return e.retryability();
        }
        Retryability::Retryable
    }

    fn is_retryable(&self) -> bool {
        self.retryability() == Retryability::Retryable
    }
}

/// Transaction lifecycle errors.
#[derive(Debug, thiserror::Error)]
pub enum TxError {
    #[error("sequence mismatch: {details}")]
    SequenceMismatch { details: String },
    #[error("simulation failed: {reason}")]
    SimulationFailed { reason: String },
    #[error("broadcast failed: {reason}")]
    BroadcastFailed { reason: String },
    #[error("transaction {tx_hash} reverted: {reason}")]
    Reverted { tx_hash: String, reason: String },
    #[error("transaction {tx_hash} not confirmed after {attempts} attempts: {reason}")]
    NotConfirmed {
        tx_hash: String,
        attempts: u32,
        reason: String,
    },
    #[error("insufficient funds: {details}")]
    InsufficientFunds { details: String },
    #[error("out of gas: {details}")]
    OutOfGas { details: String },
}

impl HasRetryability for TxError {
    fn retryability(&self) -> Retryability {
        match self {
            Self::SequenceMismatch { .. }
            | Self::SimulationFailed { .. }
            | Self::BroadcastFailed { .. }
            | Self::NotConfirmed { .. }
            | Self::OutOfGas { .. } => Retryability::Retryable,
            Self::Reverted { .. } | Self::InsufficientFunds { .. } => Retryability::Fatal,
        }
    }
}

impl TxError {
    /// Metric label for the `tx_errors` counter's `error_type` dimension.
    #[must_use]
    pub const fn metric_label(&self) -> &'static str {
        match self {
            Self::SequenceMismatch { .. } => "sequence_mismatch",
            Self::SimulationFailed { .. } => "simulate_failed",
            Self::BroadcastFailed { .. } => "broadcast_failed",
            Self::Reverted { .. } => "reverted",
            Self::NotConfirmed { .. } => "not_confirmed",
            Self::InsufficientFunds { .. } => "insufficient_funds",
            Self::OutOfGas { .. } => "out_of_gas",
        }
    }
}

/// Chain state query errors.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("query timed out: {reason}")]
    Timeout { reason: String },
    #[error("stale state: {what}")]
    StaleState { what: String },
    #[error("not found: {what}")]
    NotFound { what: String },
    #[error("deserialization failed: {reason}")]
    Deserialization { reason: String },
    #[error("unsupported type_url: {type_url}")]
    UnsupportedType { type_url: String },
}

impl HasRetryability for QueryError {
    fn retryability(&self) -> Retryability {
        match self {
            Self::Timeout { .. } | Self::StaleState { .. } => Retryability::Retryable,
            Self::NotFound { .. } | Self::Deserialization { .. } | Self::UnsupportedType { .. } => {
                Retryability::Fatal
            }
        }
    }
}

/// Proof fetch, generation, and verification errors.
#[derive(Debug, thiserror::Error)]
pub enum ProofError {
    #[error("proof fetch failed: {reason}")]
    FetchFailed { reason: String },
    #[error("ZK proving failed: {reason}")]
    ZkProvingFailed { reason: String },
    #[error("proof verification failed: {reason}")]
    VerificationFailed { reason: String },
    #[error("proof missing from response")]
    Missing,
}

impl HasRetryability for ProofError {
    fn retryability(&self) -> Retryability {
        match self {
            Self::FetchFailed { .. } | Self::ZkProvingFailed { .. } | Self::Missing => {
                Retryability::Retryable
            }
            Self::VerificationFailed { .. } => Retryability::Fatal,
        }
    }
}

/// Light client state errors.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("client {client_id} expired")]
    Expired { client_id: String },
    #[error("client {client_id} frozen")]
    Frozen { client_id: String },
    #[error("client {client_id} not found")]
    NotFound { client_id: String },
}

impl HasRetryability for ClientError {
    fn retryability(&self) -> Retryability {
        Retryability::Fatal
    }
}

/// RPC transport errors (timeout, rate limit exceeded).
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("RPC timed out after {0:?}")]
    Timeout(std::time::Duration),
}

impl HasRetryability for RpcError {
    fn retryability(&self) -> Retryability {
        match self {
            Self::Timeout(_) => Retryability::Retryable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tx_error_retryability() {
        assert_eq!(
            TxError::SequenceMismatch {
                details: "test".into()
            }
            .retryability(),
            Retryability::Retryable,
        );
        assert_eq!(
            TxError::SimulationFailed {
                reason: "test".into()
            }
            .retryability(),
            Retryability::Retryable,
        );
        assert_eq!(
            TxError::BroadcastFailed {
                reason: "test".into()
            }
            .retryability(),
            Retryability::Retryable,
        );
        assert_eq!(
            TxError::Reverted {
                tx_hash: "abc".into(),
                reason: "test".into()
            }
            .retryability(),
            Retryability::Fatal,
        );
        assert_eq!(
            TxError::NotConfirmed {
                tx_hash: "abc".into(),
                attempts: 10,
                reason: "test".into()
            }
            .retryability(),
            Retryability::Retryable,
        );
        assert_eq!(
            TxError::InsufficientFunds {
                details: "test".into()
            }
            .retryability(),
            Retryability::Fatal,
        );
        assert_eq!(
            TxError::OutOfGas {
                details: "test".into()
            }
            .retryability(),
            Retryability::Retryable,
        );
    }

    #[test]
    fn query_error_retryability() {
        assert_eq!(
            QueryError::Timeout {
                reason: "test".into()
            }
            .retryability(),
            Retryability::Retryable,
        );
        assert_eq!(
            QueryError::StaleState {
                what: "test".into()
            }
            .retryability(),
            Retryability::Retryable,
        );
        assert_eq!(
            QueryError::NotFound {
                what: "test".into()
            }
            .retryability(),
            Retryability::Fatal,
        );
        assert_eq!(
            QueryError::Deserialization {
                reason: "test".into()
            }
            .retryability(),
            Retryability::Fatal,
        );
        assert_eq!(
            QueryError::UnsupportedType {
                type_url: "test".into()
            }
            .retryability(),
            Retryability::Fatal,
        );
    }

    #[test]
    fn proof_error_retryability() {
        assert_eq!(
            ProofError::FetchFailed {
                reason: "test".into()
            }
            .retryability(),
            Retryability::Retryable,
        );
        assert_eq!(
            ProofError::ZkProvingFailed {
                reason: "test".into()
            }
            .retryability(),
            Retryability::Retryable,
        );
        assert_eq!(
            ProofError::VerificationFailed {
                reason: "test".into()
            }
            .retryability(),
            Retryability::Fatal,
        );
        assert_eq!(ProofError::Missing.retryability(), Retryability::Retryable);
    }

    #[test]
    fn client_error_retryability() {
        assert_eq!(
            ClientError::Expired {
                client_id: "test".into()
            }
            .retryability(),
            Retryability::Fatal,
        );
        assert_eq!(
            ClientError::Frozen {
                client_id: "test".into()
            }
            .retryability(),
            Retryability::Fatal,
        );
        assert_eq!(
            ClientError::NotFound {
                client_id: "test".into()
            }
            .retryability(),
            Retryability::Fatal,
        );
    }

    #[test]
    fn tx_error_metric_labels() {
        assert_eq!(
            TxError::SequenceMismatch {
                details: String::new()
            }
            .metric_label(),
            "sequence_mismatch"
        );
        assert_eq!(
            TxError::SimulationFailed {
                reason: String::new()
            }
            .metric_label(),
            "simulate_failed"
        );
        assert_eq!(
            TxError::BroadcastFailed {
                reason: String::new()
            }
            .metric_label(),
            "broadcast_failed"
        );
        assert_eq!(
            TxError::Reverted {
                tx_hash: String::new(),
                reason: String::new()
            }
            .metric_label(),
            "reverted"
        );
        assert_eq!(
            TxError::NotConfirmed {
                tx_hash: String::new(),
                attempts: 0,
                reason: String::new()
            }
            .metric_label(),
            "not_confirmed"
        );
        assert_eq!(
            TxError::InsufficientFunds {
                details: String::new()
            }
            .metric_label(),
            "insufficient_funds"
        );
        assert_eq!(
            TxError::OutOfGas {
                details: String::new()
            }
            .metric_label(),
            "out_of_gas"
        );
    }

    #[test]
    fn retryable_ext_typed_error() {
        let err: eyre::Report = TxError::Reverted {
            tx_hash: "abc".into(),
            reason: "test".into(),
        }
        .into();
        assert_eq!(err.retryability(), Retryability::Fatal);
        assert!(!err.is_retryable());
    }

    #[test]
    fn retryable_ext_untyped_error() {
        let err = eyre::eyre!("connection refused");
        assert_eq!(err.retryability(), Retryability::Retryable);
        assert!(err.is_retryable());
    }

    #[test]
    fn rpc_error_retryability() {
        use std::time::Duration;
        assert_eq!(
            RpcError::Timeout(Duration::from_secs(30)).retryability(),
            Retryability::Retryable,
        );
    }

    #[test]
    fn retryable_ext_sees_rpc_error() {
        use std::time::Duration;
        let err: eyre::Report = RpcError::Timeout(Duration::from_secs(30)).into();
        assert_eq!(err.retryability(), Retryability::Retryable);
        assert!(err.is_retryable());
    }

    #[test]
    fn retryable_ext_sees_through_wrap_err() {
        let inner: eyre::Report = TxError::InsufficientFunds {
            details: "low".into(),
        }
        .into();
        let wrapped = inner.wrap_err("sending transaction");
        assert_eq!(wrapped.retryability(), Retryability::Fatal);
    }
}
