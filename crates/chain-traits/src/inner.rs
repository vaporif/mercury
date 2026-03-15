use crate::types::{ChainTypes, IbcTypes};

/// Maps a wrapper chain type to its inner (core) chain type.
///
/// Bridge crates define wrapper types (e.g. `CosmosChain<S>`) around core types
/// (e.g. `CosmosChainInner<S>`). This trait tells the compiler the associated
/// types are identical, allowing relay code to pass values between contexts.
pub trait HasInner: ChainTypes + IbcTypes {
    type Inner: ChainTypes<
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

/// Proof data for a single packet, used by the enrichment hook.
/// Carries raw bytes so that `enrich_update_payload` impls don't need to
/// resolve chain-specific associated types through generics.
pub struct PacketProofData {
    pub sequence: u64,
    pub commitment: Vec<u8>,
    pub proof: Vec<u8>,
}
