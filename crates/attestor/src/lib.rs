//! Client for [IBC attestors](https://github.com/cosmos/ibc-attestor): signing
//! services that stand in for a light client of chains without state proofs.

pub mod aggregator;
pub mod client;
pub mod packet;
pub mod proto;

pub use aggregator::{AggregatedAttestation, Aggregator};
pub use client::{AttestorClient, CommitmentType};
pub use packet::abi_encode_packet;
