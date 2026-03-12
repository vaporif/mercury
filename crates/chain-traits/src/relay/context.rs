use mercury_core::ThreadSafe;

use crate::types::{Chain, HasIbcTypes};

pub trait Relay: ThreadSafe {
    type SrcChain: Chain<Self::DstChain>;
    type DstChain: Chain<Self::SrcChain>;

    fn src_chain(&self) -> &Self::SrcChain;
    fn dst_chain(&self) -> &Self::DstChain;
    fn src_client_id(&self) -> &<Self::SrcChain as HasIbcTypes<Self::DstChain>>::ClientId;
    fn dst_client_id(&self) -> &<Self::DstChain as HasIbcTypes<Self::SrcChain>>::ClientId;
}
