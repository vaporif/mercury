use std::fmt;
use std::sync::Arc;

pub use eyre::Report;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub struct Error {
    pub is_retryable: bool,
    pub detail: ErrorDetail,
}

#[derive(Clone)]
pub enum ErrorDetail {
    Report(Arc<Report>),
    Wrapped(String, Arc<ErrorDetail>),
}

impl Error {
    pub fn report<E: Into<Report>>(e: E) -> Self {
        Self {
            is_retryable: false,
            detail: ErrorDetail::Report(Arc::new(e.into())),
        }
    }

    pub fn retryable<E: Into<Report>>(e: E) -> Self {
        Self {
            is_retryable: true,
            detail: ErrorDetail::Report(Arc::new(e.into())),
        }
    }

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
            ErrorDetail::Report(r) => write!(f, "{r:?}"),
            ErrorDetail::Wrapped(msg, inner) => write!(f, "{msg}: {inner:?}"),
        }
    }
}

impl fmt::Display for ErrorDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorDetail::Report(r) => write!(f, "{r}"),
            ErrorDetail::Wrapped(msg, inner) => write!(f, "{msg}: {inner}"),
        }
    }
}

impl std::error::Error for Error {}
