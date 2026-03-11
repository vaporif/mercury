use async_trait::async_trait;

use mercury_chain_traits::messaging::CanSendMessages;
use mercury_chain_traits::relay::packet::{
    CanRelayAckPacket, CanRelayReceivePacket, CanRelayTimeoutPacket,
};
use mercury_chain_traits::types::{HasIbcTypes, HasMessageTypes, HasPacketTypes};
use mercury_core::error::Result;

use crate::context::RelayContext;

#[async_trait]
impl<Src, Dst> CanRelayReceivePacket for RelayContext<Src, Dst>
where
    Src: HasMessageTypes + HasIbcTypes<Dst> + HasPacketTypes<Dst> + CanSendMessages,
    Dst: HasMessageTypes + HasIbcTypes<Src> + HasPacketTypes<Src> + CanSendMessages,
{
    async fn relay_receive_packet(
        &self,
        _packet: &<Src as HasPacketTypes<Dst>>::Packet,
    ) -> Result<()> {
        // TODO: update client on dst, query commitment proof on src, build MsgRecvPacket, send to dst
        todo!("relay receive packet")
    }
}

#[async_trait]
impl<Src, Dst> CanRelayAckPacket for RelayContext<Src, Dst>
where
    Src: HasMessageTypes + HasIbcTypes<Dst> + HasPacketTypes<Dst> + CanSendMessages,
    Dst: HasMessageTypes + HasIbcTypes<Src> + HasPacketTypes<Src> + CanSendMessages,
{
    async fn relay_ack_packet(
        &self,
        _packet: &<Src as HasPacketTypes<Dst>>::Packet,
        _ack: &<Dst as HasPacketTypes<Src>>::Acknowledgement,
    ) -> Result<()> {
        // TODO: update client on src, query ack proof on dst, build MsgAcknowledgement, send to src
        todo!("relay ack packet")
    }
}

#[async_trait]
impl<Src, Dst> CanRelayTimeoutPacket for RelayContext<Src, Dst>
where
    Src: HasMessageTypes + HasIbcTypes<Dst> + HasPacketTypes<Dst> + CanSendMessages,
    Dst: HasMessageTypes + HasIbcTypes<Src> + HasPacketTypes<Src> + CanSendMessages,
{
    async fn relay_timeout_packet(
        &self,
        _packet: &<Src as HasPacketTypes<Dst>>::Packet,
    ) -> Result<()> {
        // TODO: update client on src, query non-membership receipt proof on dst, build MsgTimeout, send to src
        todo!("relay timeout packet")
    }
}
