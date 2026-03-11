use mercury_core::ThreadSafe;

use crate::types::{HasChainTypes, HasPacketTypes};

pub trait CanExtractPacketEvents<Counterparty: HasChainTypes + ?Sized>:
    HasPacketTypes<Counterparty>
{
    type SendPacketEvent: ThreadSafe;

    fn try_extract_send_packet_event(event: &Self::Event) -> Option<Self::SendPacketEvent>;
    fn packet_from_send_event(event: &Self::SendPacketEvent) -> &Self::Packet;
}
