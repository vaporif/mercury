use std::borrow::Borrow;
use std::sync::Arc;
use std::time::Duration;

use mercury_chain_traits::prelude::*;
use mercury_chain_traits::relay::Relay;
use mercury_core::error::Result;
use mercury_core::worker::spawn_worker;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::workers::clearing_worker::ClearingWorker;
use crate::workers::client_refresh::ClientRefreshWorker;
use crate::workers::event_watcher::EventWatcher;
use crate::workers::packet_worker::PacketWorker;
use crate::workers::tx_worker::{SrcTxWorker, TxWorker};

const CHANNEL_BUFFER: usize = 256;

/// Configuration for optional relay workers.
#[derive(Clone, Copy, Default)]
pub struct RelayWorkerConfig {
    pub lookback: Option<Duration>,
    pub clearing_interval: Option<Duration>,
}

/// Unidirectional relay context between a source and destination chain.
pub struct RelayContext<Src, Dst>
where
    Src: Chain<Dst>,
    Dst: Chain<Src>,
{
    pub src_chain: Src,
    pub dst_chain: Dst,
    pub src_client_id: <Src as IbcTypes<Dst>>::ClientId,
    pub dst_client_id: <Dst as IbcTypes<Src>>::ClientId,
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

    fn src_client_id(&self) -> &<Src as IbcTypes<Dst>>::ClientId {
        &self.src_client_id
    }

    fn dst_client_id(&self) -> &<Dst as IbcTypes<Src>>::ClientId {
        &self.dst_client_id
    }
}

impl<Src, Dst> RelayContext<Src, Dst>
where
    Src: Chain<Dst>,
    Dst: Chain<Src>,
    <Dst as PacketMessageBuilder<Src>>::ReceivePacketPayload: From<(
        <Src as IbcTypes<Dst>>::CommitmentProof,
        <Src as ChainTypes>::Height,
        u64,
    )>,
    <Dst as PacketMessageBuilder<Src>>::AckPacketPayload: From<(
        <Src as IbcTypes<Dst>>::CommitmentProof,
        <Src as ChainTypes>::Height,
        u64,
    )>,
    <Src as PacketMessageBuilder<Dst>>::TimeoutPacketPayload: From<(
        <Dst as IbcTypes<Src>>::CommitmentProof,
        <Dst as ChainTypes>::Height,
        u64,
    )>,
    <Src as IbcTypes<Dst>>::Packet: Borrow<<Dst as IbcTypes<Src>>::Packet>,
    <Src as IbcTypes<Dst>>::Acknowledgement: Borrow<<Dst as IbcTypes<Src>>::Acknowledgement>,
    <Dst as IbcTypes<Src>>::Acknowledgement: Borrow<<Src as IbcTypes<Dst>>::Acknowledgement>,
{
    pub async fn run_with_token(
        self: Arc<Self>,
        token: CancellationToken,
        config: RelayWorkerConfig,
    ) -> Result<()> {
        let start_height = if let Some(lookback) = config.lookback {
            let latest = self.src_chain.query_latest_height().await?;
            let block_time = self.src_chain.block_time();
            let blocks_back = (lookback.as_secs() / block_time.as_secs().max(1)).max(1);
            Src::sub_height(&latest, blocks_back)
        } else {
            None
        };

        let (event_tx, event_rx) = mpsc::channel(CHANNEL_BUFFER);
        let (tx_req_tx, tx_req_rx) = mpsc::channel(CHANNEL_BUFFER);
        let (src_tx_req_tx, src_tx_req_rx) = mpsc::channel(CHANNEL_BUFFER);

        let event_watcher = EventWatcher {
            relay: Arc::clone(&self),
            sender: event_tx.clone(),
            token: token.clone(),
            start_height,
        };

        let client_refresh = ClientRefreshWorker {
            relay: Arc::clone(&self),
            sender: tx_req_tx.clone(),
            token: token.clone(),
        };

        let packet_worker = PacketWorker {
            relay: Arc::clone(&self),
            receiver: event_rx,
            sender: tx_req_tx,
            src_sender: src_tx_req_tx,
            token: token.clone(),
        };

        let tx_worker = TxWorker {
            relay: Arc::clone(&self),
            receiver: tx_req_rx,
            token: token.clone(),
        };

        let src_tx_worker = SrcTxWorker {
            relay: Arc::clone(&self),
            receiver: src_tx_req_rx,
            token: token.clone(),
        };

        let event_watcher_handle = spawn_worker(event_watcher);
        let packet_worker_handle = spawn_worker(packet_worker);
        let tx_worker_handle = spawn_worker(tx_worker);
        let src_tx_worker_handle = spawn_worker(src_tx_worker);
        let client_refresh_handle = spawn_worker(client_refresh);

        let clearing_handle = config.clearing_interval.map_or_else(
            || tokio::spawn(futures::future::pending()),
            |interval| {
                let clearing_worker = ClearingWorker {
                    relay: Arc::clone(&self),
                    sender: event_tx,
                    token: token.clone(),
                    interval,
                };
                spawn_worker(clearing_worker)
            },
        );

        let result = tokio::select! {
            res = event_watcher_handle => res,
            res = packet_worker_handle => res,
            res = tx_worker_handle => res,
            res = src_tx_worker_handle => res,
            res = client_refresh_handle => res,
            res = clearing_handle => res,
        };

        token.cancel();

        match result {
            Ok(worker_result) => worker_result,
            Err(join_err) => Err(eyre::eyre!(join_err)),
        }
    }

    pub async fn run(self: Arc<Self>, config: RelayWorkerConfig) -> Result<()> {
        self.run_with_token(CancellationToken::new(), config).await
    }
}
