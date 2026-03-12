use crate::events::CanExtractPacketEvents;
use crate::relay::context::Relay;

/// An IBC event relevant to packet relaying.
pub enum IbcEvent<R: Relay> {
    SendPacket(<R::SrcChain as CanExtractPacketEvents<R::DstChain>>::SendPacketEvent),
    WriteAck(<R::SrcChain as CanExtractPacketEvents<R::DstChain>>::WriteAckEvent),
}
