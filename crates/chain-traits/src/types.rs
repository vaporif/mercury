use std::fmt::{Debug, Display};
use std::time::Duration;

use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::builders::{ClientMessageBuilder, PacketMessageBuilder};
use crate::events::PacketEvents;
use crate::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};

/// Core associated types for a chain: identity, messages, status, and revision.
pub trait ChainTypes: ThreadSafe {
    type Height: Clone + Ord + Debug + Display + ThreadSafe;
    type Timestamp: Clone + Ord + Debug + ThreadSafe;
    type ChainId: Clone + Debug + Display + ThreadSafe;
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
}

/// IBC-specific types relative to a counterparty chain (client, proofs, packets).
pub trait IbcTypes<Counterparty: ChainTypes + ?Sized>: ChainTypes {
    type ClientId: Clone + Debug + Display + ThreadSafe;
    type ClientState: Clone + Debug + ThreadSafe;
    type ConsensusState: Clone + Debug + ThreadSafe;
    type CommitmentProof: Clone + ThreadSafe;
    type Packet: Clone + Debug + ThreadSafe;
    type PacketCommitment: ThreadSafe;
    type PacketReceipt: ThreadSafe;
    type Acknowledgement: ThreadSafe;

    fn packet_sequence(packet: &Self::Packet) -> u64;
    fn packet_timeout_timestamp(packet: &Self::Packet) -> u64;
    fn packet_source_ports(packet: &Self::Packet) -> Vec<String>;
}

/// Sends a batch of messages to the chain.
#[async_trait]
pub trait MessageSender: ChainTypes {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<Vec<Self::MessageResponse>>;
}

/// Composite trait combining all capabilities needed for a fully functional IBC chain.
pub trait Chain<Counterparty>:
    ChainTypes
    + IbcTypes<Counterparty>
    + MessageSender
    + PacketEvents<Counterparty>
    + ChainStatusQuery
    + ClientPayloadBuilder<Counterparty>
    + ClientQuery<Counterparty>
    + ClientMessageBuilder<Counterparty>
    + PacketStateQuery<Counterparty>
    + PacketMessageBuilder<Counterparty>
where
    Counterparty: ChainTypes + IbcTypes<Self> + ClientPayloadBuilder<Self>,
{
}

impl<T, C> Chain<C> for T
where
    T: ChainTypes
        + IbcTypes<C>
        + MessageSender
        + PacketEvents<C>
        + ChainStatusQuery
        + ClientPayloadBuilder<C>
        + ClientQuery<C>
        + ClientMessageBuilder<C>
        + PacketStateQuery<C>
        + PacketMessageBuilder<C>,
    C: ChainTypes + IbcTypes<T> + ClientPayloadBuilder<T>,
{
}

// Re-export ClientPayloadBuilder from builders so Chain bounds work
pub use crate::builders::ClientPayloadBuilder;
