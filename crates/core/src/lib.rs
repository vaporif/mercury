//! Core types, error handling, and worker infrastructure for the Mercury relayer.

/// Serialization and deserialization abstractions.
pub mod encoding;
/// Error types and result aliases.
pub mod error;
/// Async worker trait and spawning utilities.
pub mod worker;

/// Marker trait for types that are `Send + Sync + 'static`.
pub trait ThreadSafe: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> ThreadSafe for T {}
