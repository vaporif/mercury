pub mod error;
pub mod runtime;
pub mod encoding;

/// Shorthand for `Send + Sync + 'static`.
pub trait ThreadSafe: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> ThreadSafe for T {}
