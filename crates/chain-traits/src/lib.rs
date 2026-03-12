//! Trait definitions for chain interaction in the Mercury relayer.
//!
//! Defines the abstract interfaces (queries, transactions, events, packet building)
//! that each chain implementation must satisfy.

/// Event extraction and block event queries.
pub mod events;
/// Builders for IBC client lifecycle messages.
pub mod message_builders;
/// Sending messages to a chain.
pub mod messaging;
/// Builders for IBC packet messages (receive, ack, timeout).
pub mod packet_builders;
/// Queries for packet commitments, receipts, and acknowledgements.
pub mod packet_queries;
/// Builders for client create/update payloads.
pub mod payload_builders;
/// Chain and client state queries.
pub mod queries;
/// Relay context traits for cross-chain relaying.
pub mod relay;
/// Transaction submission and fee estimation.
pub mod tx;
/// Core type definitions for chains, messages, and packets.
pub mod types;

pub use types::*;
