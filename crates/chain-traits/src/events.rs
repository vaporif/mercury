use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::{HasChainTypes, HasPacketTypes};

pub trait CanExtractPacketEvents<Counterparty: HasChainTypes + ?Sized>:
    HasPacketTypes<Counterparty>
{
    type SendPacketEvent: ThreadSafe;

    fn try_extract_send_packet_event(event: &Self::Event) -> Option<Self::SendPacketEvent>;
    fn packet_from_send_event(event: &Self::SendPacketEvent) -> &Self::Packet;
}

#[async_trait]
pub trait CanQueryBlockEvents: HasChainTypes {
    async fn query_block_events(&self, height: &Self::Height) -> Result<Vec<Self::Event>>;
    async fn query_latest_height(&self) -> Result<Self::Height>;
}
