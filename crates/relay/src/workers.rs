pub mod event_watcher;
pub mod packet_worker;
pub mod tx_worker;

use mercury_chain_traits::relay::context::Relay;
use mercury_chain_traits::types::HasMessageTypes;

pub struct TxRequest<R: Relay> {
    pub messages: Vec<<R::DstChain as HasMessageTypes>::Message>,
}
