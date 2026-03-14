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

/// IBC merkle prefix representing the key path in nested merkle trees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MerklePrefix(pub Vec<Vec<u8>>);

impl MerklePrefix {
    #[must_use]
    pub fn ibc_default() -> Self {
        Self(vec![b"ibc".to_vec(), b"".to_vec()])
    }
}
