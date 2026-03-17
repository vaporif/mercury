//! Core types, error handling, and worker infrastructure for the Mercury relayer.

/// Serialization and deserialization abstractions.
pub mod encoding;
/// Error types and result aliases.
pub mod error;
/// RPC rate-limiting and timeout guard.
pub mod rpc_guard;
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

/// A single membership proof entry: IBC merkle path segments, stored value, and proof bytes.
#[derive(Clone, Debug)]
pub struct MembershipProofEntry {
    pub path: Vec<Vec<u8>>,
    pub value: Vec<u8>,
    pub proof: Vec<u8>,
}

/// Collection of membership proof entries for batched proving.
#[derive(Clone, Debug, Default)]
pub struct MembershipProofs(pub Vec<MembershipProofEntry>);

impl MembershipProofs {
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn push(&mut self, entry: MembershipProofEntry) {
        self.0.push(entry);
    }
}
