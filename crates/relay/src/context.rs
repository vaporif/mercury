use std::sync::Arc;
use std::time::Duration;

use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder,
};
use mercury_chain_traits::events::PacketEvents;
use mercury_chain_traits::inner::HasInner;
use mercury_chain_traits::queries::{ClientQuery, MisbehaviourQuery};
use mercury_chain_traits::relay::{Relay, RelayChain};
use mercury_chain_traits::types::{ChainTypes, IbcTypes};
use mercury_core::error::Result;
use mercury_core::worker::spawn_worker;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::filter::PacketFilter;
use crate::workers::clearing_worker::ClearingWorker;
use crate::workers::client_refresh::ClientRefreshWorker;
use crate::workers::event_watcher::EventWatcher;
use crate::workers::misbehaviour_worker::MisbehaviourWorker;
use crate::workers::packet_worker::PacketWorker;
use crate::workers::tx_worker::{SrcTxWorker, TxWorker};

const CHANNEL_BUFFER: usize = 256;
const MAX_RESTART_BACKOFF: Duration = Duration::from_secs(60);
const INITIAL_RESTART_BACKOFF: Duration = Duration::from_secs(1);

/// Configuration for optional relay workers.
#[derive(Clone, Default)]
pub struct RelayWorkerConfig {
    pub lookback: Option<Duration>,
    pub clearing_interval: Option<Duration>,
    pub misbehaviour_scan_interval: Option<Duration>,
    pub packet_filter: Option<PacketFilter>,
}

/// Unidirectional relay context between a source and destination chain.
pub struct RelayContext<Src: ChainTypes, Dst: ChainTypes> {
    pub src_chain: Src,
    pub dst_chain: Dst,
    pub src_client_id: Src::ClientId,
    pub dst_client_id: Dst::ClientId,
}

impl<Src, Dst> Relay for RelayContext<Src, Dst>
where
    Src: RelayChain + ClientPayloadBuilder<<Dst as HasInner>::Inner> + PacketEvents,
    Dst: RelayChain
        + ClientMessageBuilder<
            <Src as HasInner>::Inner,
            CreateClientPayload = <Src as ClientPayloadBuilder<<Dst as HasInner>::Inner>>::CreateClientPayload,
            UpdateClientPayload = <Src as ClientPayloadBuilder<<Dst as HasInner>::Inner>>::UpdateClientPayload,
        > + ClientQuery<<Src as HasInner>::Inner>
        + PacketMessageBuilder<<Src as HasInner>::Inner>
        + ClientPayloadBuilder<<Src as HasInner>::Inner>,
{
    type SrcChain = Src;
    type DstChain = Dst;

    fn src_chain(&self) -> &Src {
        &self.src_chain
    }

    fn dst_chain(&self) -> &Dst {
        &self.dst_chain
    }

    fn src_client_id(&self) -> &Src::ClientId {
        &self.src_client_id
    }

    fn dst_client_id(&self) -> &Dst::ClientId {
        &self.dst_client_id
    }
}

impl<Src, Dst> RelayContext<Src, Dst>
where
    Src: RelayChain
        + ClientPayloadBuilder<<Dst as HasInner>::Inner>
        + PacketEvents
        + PacketMessageBuilder<<Dst as HasInner>::Inner>
        + ClientQuery<<Dst as HasInner>::Inner>
        + ClientMessageBuilder<
            <Dst as HasInner>::Inner,
            CreateClientPayload = <Dst as ClientPayloadBuilder<<Src as HasInner>::Inner>>::CreateClientPayload,
            UpdateClientPayload = <Dst as ClientPayloadBuilder<<Src as HasInner>::Inner>>::UpdateClientPayload,
        > + MisbehaviourDetector<<Dst as HasInner>::Inner, CounterpartyClientState = <Dst as IbcTypes>::ClientState>,
    Dst: RelayChain
        + ClientMessageBuilder<
            <Src as HasInner>::Inner,
            CreateClientPayload = <Src as ClientPayloadBuilder<<Dst as HasInner>::Inner>>::CreateClientPayload,
            UpdateClientPayload = <Src as ClientPayloadBuilder<<Dst as HasInner>::Inner>>::UpdateClientPayload,
        > + ClientQuery<<Src as HasInner>::Inner>
        + PacketEvents
        + PacketMessageBuilder<<Src as HasInner>::Inner>
        + ClientPayloadBuilder<<Src as HasInner>::Inner>
        + MisbehaviourQuery<
            <Src as HasInner>::Inner,
            CounterpartyUpdateHeader = <Src as MisbehaviourDetector<<Dst as HasInner>::Inner>>::UpdateHeader,
        > + MisbehaviourMessageBuilder<
            <Src as HasInner>::Inner,
            MisbehaviourEvidence = <Src as MisbehaviourDetector<<Dst as HasInner>::Inner>>::MisbehaviourEvidence,
        >,
{
    pub async fn run_with_token(
        self: Arc<Self>,
        token: CancellationToken,
        config: RelayWorkerConfig,
    ) -> Result<()> {
        let mut backoff = INITIAL_RESTART_BACKOFF;

        loop {
            let result = self.run_pipeline(&token, &config).await;

            if token.is_cancelled() {
                return result;
            }

            match result {
                Ok(()) => return Ok(()),
                Err(e) => {
                    warn!(
                        error = %e,
                        backoff_secs = backoff.as_secs(),
                        "relay pipeline failed, restarting"
                    );

                    tokio::select! {
                        () = token.cancelled() => return Err(e),
                        () = tokio::time::sleep(backoff) => {}
                    }

                    backoff = (backoff * 2).min(MAX_RESTART_BACKOFF);
                }
            }
        }
    }

    async fn run_pipeline(
        self: &Arc<Self>,
        token: &CancellationToken,
        config: &RelayWorkerConfig,
    ) -> Result<()> {
        // Child token so we can tear down this iteration's workers without
        // cancelling the parent shutdown token.
        let pipeline_token = token.child_token();

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
            relay: Arc::clone(self),
            sender: event_tx.clone(),
            token: pipeline_token.clone(),
            start_height,
            packet_filter: config.packet_filter.clone(),
        };

        let client_refresh = ClientRefreshWorker {
            relay: Arc::clone(self),
            sender: tx_req_tx.clone(),
            token: pipeline_token.clone(),
        };

        let packet_worker = PacketWorker {
            relay: Arc::clone(self),
            receiver: event_rx,
            sender: tx_req_tx,
            src_sender: src_tx_req_tx,
            token: pipeline_token.clone(),
        };

        let tx_worker = TxWorker {
            relay: Arc::clone(self),
            receiver: tx_req_rx,
            token: pipeline_token.clone(),
        };

        let src_tx_worker = SrcTxWorker {
            relay: Arc::clone(self),
            receiver: src_tx_req_rx,
            token: pipeline_token.clone(),
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
                    relay: Arc::clone(self),
                    sender: event_tx,
                    token: pipeline_token.clone(),
                    interval,
                    packet_filter: config.packet_filter.clone(),
                };
                spawn_worker(clearing_worker)
            },
        );

        let misbehaviour_handle = config.misbehaviour_scan_interval.map_or_else(
            || tokio::spawn(futures::future::pending()),
            |interval| {
                let misbehaviour_worker = MisbehaviourWorker {
                    relay: Arc::clone(self),
                    token: pipeline_token.clone(),
                    scan_interval: interval,
                };
                spawn_worker(misbehaviour_worker)
            },
        );

        let result = tokio::select! {
            res = event_watcher_handle => res,
            res = packet_worker_handle => res,
            res = tx_worker_handle => res,
            res = src_tx_worker_handle => res,
            res = client_refresh_handle => res,
            res = clearing_handle => res,
            res = misbehaviour_handle => res,
        };

        // Tear down remaining workers from this pipeline iteration.
        pipeline_token.cancel();

        match result {
            Ok(worker_result) => worker_result,
            Err(join_err) => Err(eyre::eyre!(join_err)),
        }
    }

    pub async fn run(self: Arc<Self>, config: RelayWorkerConfig) -> Result<()> {
        self.run_with_token(CancellationToken::new(), config).await
    }
}
