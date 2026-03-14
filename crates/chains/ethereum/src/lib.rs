//! Ethereum (EVM) chain implementation for the Mercury relayer.

/// IBC message and payload builders.
pub mod builders;
/// Ethereum chain definition and trait implementations.
pub mod chain;
/// Chain configuration.
pub mod config;
/// Contract ABI bindings generated from Eureka Solidity interfaces.
pub mod contracts;
/// Event parsing from transaction logs.
pub mod events;
/// Key management and transaction signing.
pub mod keys;
/// RPC queries and chain state.
pub mod queries;
/// Transaction building, signing, and broadcasting.
pub mod tx;
/// Core domain types for the Ethereum chain.
pub mod types;
