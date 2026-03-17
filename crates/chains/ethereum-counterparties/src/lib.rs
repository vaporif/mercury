//! Ethereum chain counterparty crate — wrapper type and cross-chain impls.

/// Wrapper type and forwarding impls for [`EthereumChain`].
pub mod wrapper;

/// Cross-chain counterparty impls for Cosmos counterparty.
#[cfg(feature = "cosmos-sp1")]
pub mod cosmos_counterparty;

pub mod plugin;

pub use mercury_ethereum::*;
pub use wrapper::EthereumAdapter;
