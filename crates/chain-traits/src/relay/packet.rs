use async_trait::async_trait;
use mercury_core::error::Result;

use super::context::Relay;
use crate::types::{HasChainTypes, HasMessageTypes, HasPacketTypes};

#[async_trait]
pub trait CanBuildReceivePacketMessages: Relay {
    async fn build_receive_packet_messages(
        &self,
        packet: &<Self::SrcChain as HasPacketTypes<Self::DstChain>>::Packet,
        proof_height: &<Self::SrcChain as HasChainTypes>::Height,
    ) -> Result<Vec<<Self::DstChain as HasMessageTypes>::Message>>;
}

#[async_trait]
pub trait CanBuildAckPacketMessages: Relay {
    async fn build_ack_packet_messages(
        &self,
        packet: &<Self::SrcChain as HasPacketTypes<Self::DstChain>>::Packet,
        ack: &<Self::DstChain as HasPacketTypes<Self::SrcChain>>::Acknowledgement,
        proof_height: &<Self::SrcChain as HasChainTypes>::Height,
    ) -> Result<Vec<<Self::DstChain as HasMessageTypes>::Message>>;
}

#[async_trait]
pub trait CanBuildTimeoutPacketMessages: Relay {
    async fn build_timeout_packet_messages(
        &self,
        packet: &<Self::SrcChain as HasPacketTypes<Self::DstChain>>::Packet,
        proof_height: &<Self::DstChain as HasChainTypes>::Height,
    ) -> Result<Vec<<Self::DstChain as HasMessageTypes>::Message>>;
}
