//! Clonable, retryable error type with context wrapping.

use std::fmt;
use std::sync::Arc;

pub use eyre::Report;

/// Alias for `std::result::Result` with [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// A clonable error that tracks whether the operation is retryable.
#[derive(Clone)]
pub struct Error {
    /// Whether the failed operation can be retried.
    pub is_retryable: bool,
    /// The underlying error detail.
    pub detail: ErrorDetail,
}

/// The inner representation of an [`Error`].
#[derive(Clone)]
pub enum ErrorDetail {
    /// A root error report.
    Report(Arc<Report>),
    /// A contextual message wrapping an inner error.
    Wrapped(String, Arc<Self>),
}

impl Error {
    /// Create a non-retryable error from any `eyre`-compatible source.
    pub fn report<E: Into<Report>>(e: E) -> Self {
        Self {
            is_retryable: false,
            detail: ErrorDetail::Report(Arc::new(e.into())),
        }
    }

    /// Create a retryable error from any `eyre`-compatible source.
    pub fn retryable<E: Into<Report>>(e: E) -> Self {
        Self {
            is_retryable: true,
            detail: ErrorDetail::Report(Arc::new(e.into())),
        }
    }

    /// Wrap this error with additional context.
    pub fn wrap(self, message: impl Into<String>) -> Self {
        Self {
            is_retryable: self.is_retryable,
            detail: ErrorDetail::Wrapped(message.into(), Arc::new(self.detail)),
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            ErrorDetail::Report(r) => write!(f, "{r:?}"),
            ErrorDetail::Wrapped(msg, inner) => write!(f, "{msg}: {inner:?}"),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            ErrorDetail::Report(r) => write!(f, "{r}"),
            ErrorDetail::Wrapped(msg, inner) => write!(f, "{msg}: {inner}"),
        }
    }
}

impl fmt::Debug for ErrorDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Report(r) => write!(f, "{r:?}"),
            Self::Wrapped(msg, inner) => write!(f, "{msg}: {inner:?}"),
        }
    }
}

impl fmt::Display for ErrorDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Report(r) => write!(f, "{r}"),
            Self::Wrapped(msg, inner) => write!(f, "{msg}: {inner}"),
        }
    }
}

impl std::error::Error for Error {}
