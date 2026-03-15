//! Ethereum chain bridge crate — wrapper type and cross-chain impls.

/// Wrapper type and forwarding impls for [`EthereumChain`].
pub mod wrapper;

/// Cross-chain bridge impls for Cosmos counterparty.
#[cfg(feature = "cosmos-sp1")]
pub mod cosmos_bridge;

pub use mercury_ethereum::*;
pub use wrapper::EthereumChain;
