use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::{HasChainTypes, HasIbcTypes};

/// Builds receive, ack, and timeout packet messages.
#[async_trait]
pub trait CanBuildPacketMessages<Counterparty>:
    HasIbcTypes<Counterparty>
where
    Counterparty: HasChainTypes + HasIbcTypes<Self>,
{
    type ReceivePacketPayload: ThreadSafe;
    type AckPacketPayload: ThreadSafe;
    type TimeoutPacketPayload: ThreadSafe;

    async fn build_receive_packet_message(
        &self,
        packet: &<Counterparty as HasIbcTypes<Self>>::Packet,
        payload: Self::ReceivePacketPayload,
    ) -> Result<Self::Message>;

    async fn build_ack_packet_message(
        &self,
        packet: &Self::Packet,
        ack: &<Counterparty as HasIbcTypes<Self>>::Acknowledgement,
        payload: Self::AckPacketPayload,
    ) -> Result<Self::Message>;

    async fn build_timeout_packet_message(
        &self,
        packet: &Self::Packet,
        payload: Self::TimeoutPacketPayload,
    ) -> Result<Self::Message>;
}
