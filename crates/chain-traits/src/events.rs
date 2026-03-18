use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::{IbcTypes, PacketSequence};

#[derive(Debug, Clone)]
pub struct BlockEvents<H, E> {
    pub height: H,
    pub events: Vec<E>,
}

pub type BlockEventStream<H, E> = Pin<Box<dyn Stream<Item = Result<BlockEvents<H, E>>> + Send>>;

/// Extracts IBC packet events from raw chain events and queries block events.
#[async_trait]
pub trait PacketEvents: IbcTypes {
    type SendPacketEvent: ThreadSafe;
    type WriteAckEvent: ThreadSafe;

    fn try_extract_send_packet_event(event: &Self::Event) -> Option<Self::SendPacketEvent>;
    fn try_extract_write_ack_event(event: &Self::Event) -> Option<Self::WriteAckEvent>;
    fn packet_from_send_event(event: &Self::SendPacketEvent) -> &Self::Packet;
    fn packet_from_write_ack_event(
        event: &Self::WriteAckEvent,
    ) -> (&Self::Packet, &Self::Acknowledgement);
    async fn query_block_events(&self, height: &Self::Height) -> Result<Vec<Self::Event>>;

    async fn query_send_packet_event(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
    ) -> Result<Option<Self::SendPacketEvent>>;

    async fn subscribe_block_events(
        &self,
    ) -> Result<Option<BlockEventStream<Self::Height, Self::Event>>> {
        Ok(None)
    }
}
