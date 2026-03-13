//! Trait definitions for chain interaction in the Mercury relayer.
//!
//! Defines the abstract interfaces (queries, transactions, events, building)
//! that each chain implementation must satisfy.

/// Builders for IBC client and packet messages and payloads.
pub mod builders;
/// Event extraction and block event queries.
pub mod events;
/// Chain, client, and packet state queries.
pub mod queries;
/// Relay context traits for cross-chain relaying.
pub mod relay;
/// Transaction submission and fee estimation.
pub mod tx;
/// Core type definitions for chains, messages, and packets.
pub mod types;

/// Re-exports all traits bundled into `Chain`.
pub mod prelude;

pub use types::*;
