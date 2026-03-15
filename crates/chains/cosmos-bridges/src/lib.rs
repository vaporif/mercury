//! Cosmos SDK chain bridge crate — wrapper type and cross-chain impls.

/// Wrapper type and forwarding impls for [`CosmosChain`].
pub mod wrapper;

/// Cross-chain bridge impls for Ethereum counterparty.
#[cfg(feature = "ethereum-beacon")]
pub mod ethereum_bridge;

pub use mercury_cosmos::*;
pub use wrapper::CosmosChain;
