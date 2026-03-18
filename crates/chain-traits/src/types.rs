use std::fmt::{Debug, Display};
use std::time::Duration;

use async_trait::async_trait;
use mercury_core::error::Result;
use mercury_core::{ChainLabel, ThreadSafe};

/// IBC packet sequence number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PacketSequence(pub u64);

impl Display for PacketSequence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for PacketSequence {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<PacketSequence> for u64 {
    fn from(v: PacketSequence) -> Self {
        v.0
    }
}

/// Packet timeout as a UNIX timestamp in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeoutTimestamp(pub u64);

impl Display for TimeoutTimestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for TimeoutTimestamp {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<TimeoutTimestamp> for u64 {
    fn from(v: TimeoutTimestamp) -> Self {
        v.0
    }
}

/// IBC port identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(transparent)]
pub struct Port(pub String);

impl Display for Port {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Port {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for Port {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<Port> for String {
    fn from(v: Port) -> Self {
        v.0
    }
}

/// Core associated types for a chain: identity, messages, status, and revision.
pub trait ChainTypes: ThreadSafe {
    type Height: Clone + Ord + Debug + Display + ThreadSafe;
    type Timestamp: Clone + Ord + Debug + ThreadSafe;
    type ChainId: Clone + Debug + Display + ThreadSafe;
    type ClientId: Clone + Debug + Display + ThreadSafe;
    type Event: Clone + Debug + ThreadSafe;
    type Message: ThreadSafe;
    type MessageResponse: ThreadSafe;
    type ChainStatus: ThreadSafe;

    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height;
    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp;
    fn chain_status_timestamp_secs(status: &Self::ChainStatus) -> u64;
    fn revision_number(&self) -> u64;
    fn increment_height(height: &Self::Height) -> Option<Self::Height>;
    fn sub_height(height: &Self::Height, n: u64) -> Option<Self::Height>;
    fn block_time(&self) -> Duration;
    // TODO: make Option — Solana has no chain ID
    fn chain_id(&self) -> &Self::ChainId;
    fn chain_label(&self) -> ChainLabel;
}

/// IBC-specific types relative to a counterparty chain (client, proofs, packets).
pub trait IbcTypes: ChainTypes {
    type ClientState: Clone + Debug + ThreadSafe;
    type ConsensusState: Clone + Debug + ThreadSafe;
    type CommitmentProof: Clone + ThreadSafe;
    type Packet: Clone + Debug + ThreadSafe;
    type PacketCommitment: ThreadSafe;
    type PacketReceipt: ThreadSafe;
    type Acknowledgement: ThreadSafe;

    fn packet_sequence(packet: &Self::Packet) -> PacketSequence;
    fn packet_timeout_timestamp(packet: &Self::Packet) -> TimeoutTimestamp;
    fn packet_source_ports(packet: &Self::Packet) -> Vec<Port>;
}

/// Receipt from a confirmed transaction batch.
#[derive(Clone, Debug)]
pub struct TxReceipt {
    /// Total gas consumed (if available from chain).
    pub gas_used: Option<u64>,
    /// When the transaction was confirmed in a block.
    pub confirmed_at: std::time::Instant,
}

/// Sends a batch of messages to the chain.
#[async_trait]
pub trait MessageSender: ChainTypes {
    async fn send_messages(&self, messages: Vec<Self::Message>) -> Result<TxReceipt>;
}
