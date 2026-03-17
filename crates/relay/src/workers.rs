pub mod client_refresh;
pub mod event_watcher;
pub mod misbehaviour_worker;
/// Scans for unrelayed packet commitments and recovers missed events.
pub mod packet_sweeper;
pub mod packet_worker;
pub mod tx_worker;

use std::time::Instant;

use mercury_chain_traits::relay::Relay;
use mercury_chain_traits::types::ChainTypes;

/// Messages plus a timestamp for TX latency tracking.
pub struct TimestampedMessages<M> {
    pub messages: Vec<M>,
    pub created_at: Instant,
}

/// A batch of messages to submit to the destination chain.
pub struct DstTxRequest<R: Relay> {
    pub messages: Vec<<R::DstChain as ChainTypes>::Message>,
    pub created_at: Instant,
}

impl<R: Relay> From<DstTxRequest<R>> for TimestampedMessages<<R::DstChain as ChainTypes>::Message> {
    fn from(req: DstTxRequest<R>) -> Self {
        Self {
            messages: req.messages,
            created_at: req.created_at,
        }
    }
}

/// A batch of messages to submit to the source chain.
pub struct SrcTxRequest<R: Relay> {
    pub messages: Vec<<R::SrcChain as ChainTypes>::Message>,
    pub created_at: Instant,
}

impl<R: Relay> From<SrcTxRequest<R>> for TimestampedMessages<<R::SrcChain as ChainTypes>::Message> {
    fn from(req: SrcTxRequest<R>) -> Self {
        Self {
            messages: req.messages,
            created_at: req.created_at,
        }
    }
}
