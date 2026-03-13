use std::fmt::{Debug, Display};

use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::builders::{CanBuildClientMessages, CanBuildPacketMessages};
use crate::events::{CanExtractPacketEvents, CanQueryBlockEvents};
use crate::queries::{CanQueryChainStatus, CanQueryClient, CanQueryPacketState};

/// Core associated types for a chain: identity, messages, status, and revision.
pub trait HasChainTypes: ThreadSafe {
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
}

/// IBC-specific types relative to a counterparty chain (client, proofs, packets).
pub trait HasIbcTypes<Counterparty: HasChainTypes + ?Sized>: HasChainTypes {
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
}

/// Sends a batch of messages to the chain.
#[async_trait]
pub trait CanSendMessages: HasChainTypes {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<Vec<Self::MessageResponse>>;
}

/// Composite trait combining all capabilities needed for a fully functional IBC chain.
pub trait Chain<Counterparty>:
    HasChainTypes
    + HasIbcTypes<Counterparty>
    + CanSendMessages
    + CanExtractPacketEvents<Counterparty>
    + CanQueryChainStatus
    + CanQueryBlockEvents
    + CanBuildClientPayloads<Counterparty>
    + CanQueryClient<Counterparty>
    + CanBuildClientMessages<Counterparty>
    + CanQueryPacketState<Counterparty>
    + CanBuildPacketMessages<Counterparty>
where
    Counterparty: HasChainTypes + HasIbcTypes<Self> + CanBuildClientPayloads<Self>,
{
}

impl<T, C> Chain<C> for T
where
    T: HasChainTypes
        + HasIbcTypes<C>
        + CanSendMessages
        + CanExtractPacketEvents<C>
        + CanQueryChainStatus
        + CanQueryBlockEvents
        + CanBuildClientPayloads<C>
        + CanQueryClient<C>
        + CanBuildClientMessages<C>
        + CanQueryPacketState<C>
        + CanBuildPacketMessages<C>,
    C: HasChainTypes + HasIbcTypes<T> + CanBuildClientPayloads<T>,
{
}

// Re-export CanBuildClientPayloads from builders so Chain bounds work
pub use crate::builders::CanBuildClientPayloads;
