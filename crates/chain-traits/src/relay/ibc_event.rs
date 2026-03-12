use crate::events::CanExtractPacketEvents;
use crate::relay::context::Relay;

pub enum IbcEvent<R: Relay> {
    SendPacket(<R::SrcChain as CanExtractPacketEvents<R::DstChain>>::SendPacketEvent),
    WriteAck(<R::SrcChain as CanExtractPacketEvents<R::DstChain>>::WriteAckEvent),
}
