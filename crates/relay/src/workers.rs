/// Scans for unrelayed packet commitments and recovers missed events.
pub mod clearing_worker;
/// Periodic IBC client refresh worker.
pub mod client_refresh;
/// Source chain block event poller.
pub mod event_watcher;
/// Packet proof building and message construction.
pub mod packet_worker;
/// Transaction submission workers for source and destination chains.
pub mod tx_worker;

use mercury_chain_traits::relay::Relay;
use mercury_chain_traits::types::ChainTypes;

/// A batch of messages to submit to the destination chain.
pub struct DstTxRequest<R: Relay> {
    pub messages: Vec<<R::DstChain as ChainTypes>::Message>,
}

impl<R: Relay> From<DstTxRequest<R>> for Vec<<R::DstChain as ChainTypes>::Message> {
    fn from(req: DstTxRequest<R>) -> Self {
        req.messages
    }
}

/// A batch of messages to submit to the source chain.
pub struct SrcTxRequest<R: Relay> {
    pub messages: Vec<<R::SrcChain as ChainTypes>::Message>,
}

impl<R: Relay> From<SrcTxRequest<R>> for Vec<<R::SrcChain as ChainTypes>::Message> {
    fn from(req: SrcTxRequest<R>) -> Self {
        req.messages
    }
}
