use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::{HasChainTypes, HasPacketTypes};

pub trait CanExtractPacketEvents<Counterparty: HasChainTypes + ?Sized>:
    HasPacketTypes<Counterparty>
{
    type SendPacketEvent: ThreadSafe;
    type WriteAckEvent: ThreadSafe;

    fn try_extract_send_packet_event(event: &Self::Event) -> Option<Self::SendPacketEvent>;
    fn try_extract_write_ack_event(event: &Self::Event) -> Option<Self::WriteAckEvent>;
    fn packet_from_send_event(event: &Self::SendPacketEvent) -> &Self::Packet;
    fn packet_from_write_ack_event(
        event: &Self::WriteAckEvent,
    ) -> (&Self::Packet, &Self::Acknowledgement);
}

#[async_trait]
pub trait CanQueryBlockEvents: HasChainTypes {
    async fn query_block_events(&self, height: &Self::Height) -> Result<Vec<Self::Event>>;
    async fn query_latest_height(&self) -> Result<Self::Height>;
    fn increment_height(height: &Self::Height) -> Option<Self::Height>;
}
