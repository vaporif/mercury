/// Periodic IBC client refresh worker.
pub mod client_refresh;
/// Source chain block event poller.
pub mod event_watcher;
/// Packet proof building and message construction.
pub mod packet_worker;
/// Transaction submission workers for source and destination chains.
pub mod tx_worker;

use mercury_chain_traits::relay::context::Relay;
use mercury_chain_traits::types::HasChainTypes;

/// A batch of messages to submit to the destination chain.
pub struct DstTxRequest<R: Relay> {
    pub messages: Vec<<R::DstChain as HasChainTypes>::Message>,
}

impl<R: Relay> From<DstTxRequest<R>> for Vec<<R::DstChain as HasChainTypes>::Message> {
    fn from(req: DstTxRequest<R>) -> Self {
        req.messages
    }
}

/// A batch of messages to submit to the source chain.
pub struct SrcTxRequest<R: Relay> {
    pub messages: Vec<<R::SrcChain as HasChainTypes>::Message>,
}

impl<R: Relay> From<SrcTxRequest<R>> for Vec<<R::SrcChain as HasChainTypes>::Message> {
    fn from(req: SrcTxRequest<R>) -> Self {
        req.messages
    }
}
