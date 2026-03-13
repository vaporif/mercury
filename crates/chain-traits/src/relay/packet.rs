use async_trait::async_trait;
use mercury_core::error::Result;

use super::context::Relay;
use crate::types::{HasChainTypes, HasMessageTypes, HasPacketTypes};

/// Builds relay-level packet messages (receive, ack, timeout).
#[async_trait]
pub trait CanBuildRelayPacketMessages: Relay {
    async fn build_receive_packet_messages(
        &self,
        packet: &<Self::SrcChain as HasPacketTypes<Self::DstChain>>::Packet,
        proof_height: &<Self::SrcChain as HasChainTypes>::Height,
    ) -> Result<Vec<<Self::DstChain as HasMessageTypes>::Message>>;

    async fn build_ack_packet_messages(
        &self,
        packet: &<Self::SrcChain as HasPacketTypes<Self::DstChain>>::Packet,
        ack: &<Self::DstChain as HasPacketTypes<Self::SrcChain>>::Acknowledgement,
        proof_height: &<Self::SrcChain as HasChainTypes>::Height,
    ) -> Result<Vec<<Self::DstChain as HasMessageTypes>::Message>>;

    /// Build timeout messages to submit to the **source** chain.
    /// The proof of non-receipt comes from the destination chain.
    async fn build_timeout_packet_messages(
        &self,
        packet: &<Self::SrcChain as HasPacketTypes<Self::DstChain>>::Packet,
        proof_height: &<Self::DstChain as HasChainTypes>::Height,
    ) -> Result<Vec<<Self::SrcChain as HasMessageTypes>::Message>>;
}
