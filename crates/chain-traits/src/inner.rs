use crate::types::{ChainTypes, IbcTypes};

/// Maps a wrapper chain type to its inner (core) chain type.
///
/// Bridge crates define wrapper types (e.g. `CosmosChain<S>`) around core types
/// (e.g. `CosmosChainInner<S>`). This trait tells the compiler the associated
/// types are identical, allowing relay code to pass values between contexts.
///
/// Wrappers exist because of Rust's orphan rule: cross-chain trait impls
/// (e.g. `ClientMessageBuilder<CosmosChainInner>` for `EthereumChain`) must live
/// in the bridge crate, which can only impl traits on locally-defined types.
/// This is the cost of the multi-crate design — in exchange we get independent
/// compilation, feature gating, and additive chain pairs without touching
/// existing chain crates. See `docs/architecture.md`.
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
