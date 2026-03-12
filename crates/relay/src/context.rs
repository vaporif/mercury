use std::sync::Arc;

use mercury_chain_traits::relay::Relay;
use mercury_chain_traits::types::{Chain, HasIbcTypes};
use mercury_core::error::Result;
use mercury_core::worker::spawn_worker;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::workers::event_watcher::EventWatcher;
use crate::workers::packet_worker::PacketWorker;
use crate::workers::tx_worker::TxWorker;

const CHANNEL_BUFFER: usize = 256;

pub struct RelayContext<Src, Dst>
where
    Src: Chain<Dst>,
    Dst: Chain<Src>,
{
    pub src_chain: Src,
    pub dst_chain: Dst,
    pub src_client_id: <Src as HasIbcTypes<Dst>>::ClientId,
    pub dst_client_id: <Dst as HasIbcTypes<Src>>::ClientId,
}

impl<Src, Dst> Relay for RelayContext<Src, Dst>
where
    Src: Chain<Dst>,
    Dst: Chain<Src>,
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

impl<Src, Dst> RelayContext<Src, Dst>
where
    Src: Chain<Dst>,
    Dst: Chain<Src>,
{
    pub async fn run(self: Arc<Self>) -> Result<()> {
        let token = CancellationToken::new();

        let (event_tx, event_rx) = mpsc::channel(CHANNEL_BUFFER);
        let (tx_req_tx, tx_req_rx) = mpsc::channel(CHANNEL_BUFFER);

        let event_watcher = EventWatcher {
            relay: Arc::clone(&self),
            sender: event_tx,
            token: token.clone(),
        };
        let packet_worker = PacketWorker {
            relay: Arc::clone(&self),
            receiver: event_rx,
            sender: tx_req_tx,
            token: token.clone(),
        };
        let tx_worker = TxWorker {
            relay: Arc::clone(&self),
            receiver: tx_req_rx,
            token: token.clone(),
        };

        let event_watcher_handle = spawn_worker(event_watcher);
        let packet_worker_handle = spawn_worker(packet_worker);
        let tx_worker_handle = spawn_worker(tx_worker);

        let result = tokio::select! {
            res = event_watcher_handle => res,
            res = packet_worker_handle => res,
            res = tx_worker_handle => res,
        };

        token.cancel();

        match result {
            Ok(worker_result) => worker_result,
            Err(join_err) => Err(mercury_core::error::Error::report(join_err)),
        }
    }
}
