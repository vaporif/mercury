use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::{HasChainTypes, HasMessageTypes, HasPacketTypes};

/// Builds a receive-packet message from a counterparty packet and proof payload.
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

/// Builds an acknowledgement-packet message from a packet, ack, and proof payload.
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

/// Builds a timeout-packet message from a packet and proof payload.
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
