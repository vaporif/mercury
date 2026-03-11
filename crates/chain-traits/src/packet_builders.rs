use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::{HasChainTypes, HasMessageTypes, HasPacketTypes};

#[async_trait]
pub trait CanBuildReceivePacketMessage<Counterparty>:
    HasMessageTypes + HasPacketTypes<Counterparty>
where
    Counterparty: HasChainTypes + HasPacketTypes<Self>,
{
    type ReceivePacketPayload: ThreadSafe;
    async fn build_receive_packet_message(
        &self,
        packet: &<Counterparty as HasPacketTypes<Self>>::Packet,
        payload: Self::ReceivePacketPayload,
    ) -> Result<Self::Message>;
}

#[async_trait]
pub trait CanBuildAckPacketMessage<Counterparty>:
    HasMessageTypes + HasPacketTypes<Counterparty>
where
    Counterparty: HasChainTypes + HasPacketTypes<Self>,
{
    type AckPacketPayload: ThreadSafe;
    async fn build_ack_packet_message(
        &self,
        packet: &Self::Packet,
        ack: &<Counterparty as HasPacketTypes<Self>>::Acknowledgement,
        payload: Self::AckPacketPayload,
    ) -> Result<Self::Message>;
}

#[async_trait]
pub trait CanBuildTimeoutPacketMessage<Counterparty>:
    HasMessageTypes + HasPacketTypes<Counterparty>
where
    Counterparty: HasChainTypes + HasPacketTypes<Self>,
{
    type TimeoutPacketPayload: ThreadSafe;
    async fn build_timeout_packet_message(
        &self,
        packet: &Self::Packet,
        payload: Self::TimeoutPacketPayload,
    ) -> Result<Self::Message>;
}
