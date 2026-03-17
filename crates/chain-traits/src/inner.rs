use crate::types::{ChainTypes, IbcTypes};

pub type Core<T> = <T as HasCore>::Core;

// Orphan rule avoidance
pub trait HasCore: ChainTypes + IbcTypes {
    type Core: ChainTypes<
            Height = Self::Height,
            Timestamp = Self::Timestamp,
            ChainId = Self::ChainId,
            ClientId = Self::ClientId,
            Event = Self::Event,
            Message = Self::Message,
            MessageResponse = Self::MessageResponse,
            ChainStatus = Self::ChainStatus,
        > + IbcTypes<
            ClientState = Self::ClientState,
            ConsensusState = Self::ConsensusState,
            CommitmentProof = Self::CommitmentProof,
            Packet = Self::Packet,
            PacketCommitment = Self::PacketCommitment,
            PacketReceipt = Self::PacketReceipt,
            Acknowledgement = Self::Acknowledgement,
        >;
}
