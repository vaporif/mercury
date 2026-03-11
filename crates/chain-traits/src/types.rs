use std::fmt::{Debug, Display};

use mercury_core::ThreadSafe;

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
}

pub trait HasChainStatusType: HasChainTypes {
    type ChainStatus: ThreadSafe;
    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height;
    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp;
}
