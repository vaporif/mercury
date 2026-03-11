use async_trait::async_trait;
use mercury_core::error::Result;

use crate::types::HasPacketTypes;
use super::context::Relay;

#[async_trait]
pub trait CanRelayReceivePacket: Relay {
    async fn relay_receive_packet(
        &self,
        packet: &<Self::SrcChain as HasPacketTypes<Self::DstChain>>::Packet,
    ) -> Result<()>;
}

#[async_trait]
pub trait CanRelayAckPacket: Relay {
    async fn relay_ack_packet(
        &self,
        packet: &<Self::SrcChain as HasPacketTypes<Self::DstChain>>::Packet,
        ack: &<Self::DstChain as HasPacketTypes<Self::SrcChain>>::Acknowledgement,
    ) -> Result<()>;
}

#[async_trait]
pub trait CanRelayTimeoutPacket: Relay {
    async fn relay_timeout_packet(
        &self,
        packet: &<Self::SrcChain as HasPacketTypes<Self::DstChain>>::Packet,
    ) -> Result<()>;
}
