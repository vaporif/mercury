pub mod client_refresh;
pub mod event_watcher;
pub mod packet_worker;
pub mod tx_worker;

use mercury_chain_traits::relay::context::Relay;
use mercury_chain_traits::types::HasMessageTypes;

pub struct DstTxRequest<R: Relay> {
    pub messages: Vec<<R::DstChain as HasMessageTypes>::Message>,
}

impl<R: Relay> From<DstTxRequest<R>> for Vec<<R::DstChain as HasMessageTypes>::Message> {
    fn from(req: DstTxRequest<R>) -> Self {
        req.messages
    }
}

pub struct SrcTxRequest<R: Relay> {
    pub messages: Vec<<R::SrcChain as HasMessageTypes>::Message>,
}

impl<R: Relay> From<SrcTxRequest<R>> for Vec<<R::SrcChain as HasMessageTypes>::Message> {
    fn from(req: SrcTxRequest<R>) -> Self {
        req.messages
    }
}
