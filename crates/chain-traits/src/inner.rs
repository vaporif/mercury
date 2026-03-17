use crate::types::{ChainTypes, IbcTypes};

/// Maps an adapter (wrapper) chain type to its core chain type.
///
/// Bridge crates define adapter types (e.g. `CosmosAdapter<S>`) around core types
/// (e.g. `CosmosChain<S>`). This trait tells the compiler the associated
/// types are identical, allowing relay code to pass values between contexts.
///
/// Adapters exist because of Rust's orphan rule: cross-chain trait impls
/// (e.g. `ClientMessageBuilder<CosmosChain>` for `EthereumAdapter`) must live
/// in the bridge crate, which can only impl traits on locally-defined types.
/// This is the cost of the multi-crate design — in exchange we get independent
/// compilation, feature gating, and additive chain pairs without touching
/// existing chain crates. See `docs/architecture.md`.
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
