use mercury_core::ThreadSafe;

use crate::messaging::CanSendMessages;
use crate::types::{HasIbcTypes, HasMessageTypes, HasPacketTypes};

pub trait Relay: ThreadSafe {
    type SrcChain: HasMessageTypes
        + HasIbcTypes<Self::DstChain>
        + HasPacketTypes<Self::DstChain>
        + CanSendMessages;
    type DstChain: HasMessageTypes
        + HasIbcTypes<Self::SrcChain>
        + HasPacketTypes<Self::SrcChain>
        + CanSendMessages;

    fn src_chain(&self) -> &Self::SrcChain;
    fn dst_chain(&self) -> &Self::DstChain;
    fn src_client_id(&self) -> &<Self::SrcChain as HasIbcTypes<Self::DstChain>>::ClientId;
    fn dst_client_id(&self) -> &<Self::DstChain as HasIbcTypes<Self::SrcChain>>::ClientId;
}
