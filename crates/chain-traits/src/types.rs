use std::fmt::{Debug, Display};

use mercury_core::ThreadSafe;

use crate::events::CanExtractPacketEvents;
use crate::messaging::CanSendMessages;
use crate::queries::CanQueryChainStatus;

pub trait HasChainTypes: ThreadSafe {
    type Height: Clone + Ord + Debug + Display + ThreadSafe;
    type Timestamp: Clone + Ord + Debug + ThreadSafe;
    type ChainId: Clone + Debug + Display + ThreadSafe;
    type Event: Clone + Debug + ThreadSafe;
}

pub trait HasMessageTypes: HasChainTypes {
    type Message: ThreadSafe;
    type MessageResponse: ThreadSafe;
}

pub trait HasIbcTypes<Counterparty: HasChainTypes + ?Sized>: HasChainTypes {
    type ClientId: Clone + Debug + Display + ThreadSafe;
    type ClientState: Clone + Debug + ThreadSafe;
    type ConsensusState: Clone + Debug + ThreadSafe;
    type CommitmentProof: Clone + ThreadSafe;
}

pub trait HasPacketTypes<Counterparty: HasChainTypes + ?Sized>: HasIbcTypes<Counterparty> {
    type Packet: Clone + Debug + ThreadSafe;
    type PacketCommitment: ThreadSafe;
    type PacketReceipt: ThreadSafe;
    type Acknowledgement: ThreadSafe;

    fn packet_sequence(packet: &Self::Packet) -> u64;
    fn packet_timeout_timestamp(packet: &Self::Packet) -> u64;
}

pub trait Chain<Counterparty: HasChainTypes + ?Sized>:
    HasMessageTypes
    + HasPacketTypes<Counterparty>
    + CanSendMessages
    + CanExtractPacketEvents<Counterparty>
    + CanQueryChainStatus
{
}

impl<T, C> Chain<C> for T
where
    T: HasMessageTypes
        + HasPacketTypes<C>
        + CanSendMessages
        + CanExtractPacketEvents<C>
        + CanQueryChainStatus,
    C: HasChainTypes + ?Sized,
{
}

pub trait HasChainStatusType: HasChainTypes {
    type ChainStatus: ThreadSafe;
    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height;
    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp;
    fn chain_status_timestamp_secs(status: &Self::ChainStatus) -> u64;
}

pub trait HasRevisionNumber: HasChainTypes {
    fn revision_number(&self) -> u64;
}
