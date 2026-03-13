//! IBC v2 relay pipeline — bidirectional relaying, client updates, and packet workers.

/// Bidirectional relay context.
pub mod birelay;
/// IBC light client operations.
pub mod client;
/// Unidirectional relay context and pipeline orchestration.
pub mod context;
/// Packet filtering by source port with allow/deny policies.
pub mod filter;
/// IBC packet relay logic.
pub mod packet;
/// Background worker tasks for the relay pipeline.
pub mod workers;

#[cfg(test)]
pub(crate) mod testhelpers;
