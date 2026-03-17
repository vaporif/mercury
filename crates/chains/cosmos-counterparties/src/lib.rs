//! Cosmos SDK chain counterparty crate — wrapper type and cross-chain impls.

/// Wrapper type and forwarding impls for [`CosmosAdapter`].
pub mod wrapper;

/// Cross-chain counterparty impls for Ethereum counterparty.
#[cfg(feature = "ethereum-beacon")]
pub mod ethereum_counterparty;

pub use mercury_cosmos::*;
pub use wrapper::CosmosAdapter;
