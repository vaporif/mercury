use mercury_chain_traits::messaging::CanSendMessages;
use mercury_chain_traits::relay::Relay;
use mercury_chain_traits::types::{HasIbcTypes, HasMessageTypes, HasPacketTypes};

pub struct RelayContext<Src, Dst>
where
    Src: HasMessageTypes + HasIbcTypes<Dst> + HasPacketTypes<Dst> + CanSendMessages,
    Dst: HasMessageTypes + HasIbcTypes<Src> + HasPacketTypes<Src> + CanSendMessages,
{
    pub src_chain: Src,
    pub dst_chain: Dst,
    pub src_client_id: <Src as HasIbcTypes<Dst>>::ClientId,
    pub dst_client_id: <Dst as HasIbcTypes<Src>>::ClientId,
}

impl<Src, Dst> Relay for RelayContext<Src, Dst>
where
    Src: HasMessageTypes + HasIbcTypes<Dst> + HasPacketTypes<Dst> + CanSendMessages,
    Dst: HasMessageTypes + HasIbcTypes<Src> + HasPacketTypes<Src> + CanSendMessages,
{
    type SrcChain = Src;
    type DstChain = Dst;

    fn src_chain(&self) -> &Src {
        &self.src_chain
    }

    fn dst_chain(&self) -> &Dst {
        &self.dst_chain
    }

    fn src_client_id(&self) -> &<Src as HasIbcTypes<Dst>>::ClientId {
        &self.src_client_id
    }

    fn dst_client_id(&self) -> &<Dst as HasIbcTypes<Src>>::ClientId {
        &self.dst_client_id
    }
}
