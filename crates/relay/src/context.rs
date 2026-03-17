use std::sync::Arc;
use std::time::Duration;

use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder,
};
use mercury_chain_traits::events::PacketEvents;
use mercury_chain_traits::inner::HasCore;
use mercury_chain_traits::queries::{ClientQuery, MisbehaviourQuery};
use mercury_chain_traits::relay::{Relay, RelayChain};
use mercury_chain_traits::types::{ChainTypes, IbcTypes};
use mercury_core::error::Result;
use mercury_core::worker::spawn_worker;
use mercury_telemetry::guard::WorkerGuard;
use mercury_telemetry::recorder::{
    ClearingMetrics, ClientMetrics, EventMetrics, MisbehaviourMetrics, PacketMetrics, TxDirection,
    TxMetrics,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::filter::PacketFilter;
use crate::workers::client_refresh::ClientRefreshWorker;
use crate::workers::event_watcher::EventWatcher;
use crate::workers::misbehaviour_worker::MisbehaviourWorker;
use crate::workers::packet_sweeper::PacketSweeper;
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
    Src: RelayChain + ClientPayloadBuilder<<Dst as HasCore>::Core> + PacketEvents,
    Dst: RelayChain
        + ClientMessageBuilder<
            <Src as HasCore>::Core,
            CreateClientPayload = <Src as ClientPayloadBuilder<<Dst as HasCore>::Core>>::CreateClientPayload,
            UpdateClientPayload = <Src as ClientPayloadBuilder<<Dst as HasCore>::Core>>::UpdateClientPayload,
        > + ClientQuery<<Src as HasCore>::Core>
        + PacketMessageBuilder<<Src as HasCore>::Core>
        + ClientPayloadBuilder<<Src as HasCore>::Core>,
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
        + ClientPayloadBuilder<<Dst as HasCore>::Core>
        + PacketEvents
        + PacketMessageBuilder<<Dst as HasCore>::Core>
        + ClientQuery<<Dst as HasCore>::Core>
        + ClientMessageBuilder<
            <Dst as HasCore>::Core,
            CreateClientPayload = <Dst as ClientPayloadBuilder<<Src as HasCore>::Core>>::CreateClientPayload,
            UpdateClientPayload = <Dst as ClientPayloadBuilder<<Src as HasCore>::Core>>::UpdateClientPayload,
        > + MisbehaviourDetector<<Dst as HasCore>::Core, CounterpartyClientState = <Dst as IbcTypes>::ClientState>,
    Dst: RelayChain
        + ClientMessageBuilder<
            <Src as HasCore>::Core,
            CreateClientPayload = <Src as ClientPayloadBuilder<<Dst as HasCore>::Core>>::CreateClientPayload,
            UpdateClientPayload = <Src as ClientPayloadBuilder<<Dst as HasCore>::Core>>::UpdateClientPayload,
        > + ClientQuery<<Src as HasCore>::Core>
        + PacketEvents
        + PacketMessageBuilder<<Src as HasCore>::Core>
        + ClientPayloadBuilder<<Src as HasCore>::Core>
        + MisbehaviourQuery<
            <Src as HasCore>::Core,
            CounterpartyUpdateHeader = <Src as MisbehaviourDetector<<Dst as HasCore>::Core>>::UpdateHeader,
        > + MisbehaviourMessageBuilder<
            <Src as HasCore>::Core,
            MisbehaviourEvidence = <Src as MisbehaviourDetector<<Dst as HasCore>::Core>>::MisbehaviourEvidence,
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

    #[allow(clippy::too_many_lines)]
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

        let src_label = self.src_chain.chain_label();
        let dst_label = self.dst_chain.chain_label();

        let event_watcher = EventWatcher {
            relay: Arc::clone(self),
            sender: event_tx.clone(),
            token: pipeline_token.clone(),
            start_height,
            packet_filter: config.packet_filter.clone(),
            metrics: EventMetrics::new(src_label.clone())
                .with_counterparty(dst_label.clone()),
        };

        let client_refresh = ClientRefreshWorker {
            relay: Arc::clone(self),
            sender: tx_req_tx.clone(),
            token: pipeline_token.clone(),
            metrics: ClientMetrics::new(src_label.clone())
                .with_counterparty(dst_label.clone())
                .with_client_id(self.dst_client_id.to_string()),
        };

        let packet_worker = PacketWorker {
            relay: Arc::clone(self),
            receiver: event_rx,
            sender: tx_req_tx,
            src_sender: src_tx_req_tx,
            token: pipeline_token.clone(),
            metrics: PacketMetrics::new(src_label.clone())
                .with_counterparty(dst_label.clone()),
        };

        let tx_worker = TxWorker {
            relay: Arc::clone(self),
            receiver: tx_req_rx,
            token: pipeline_token.clone(),
            metrics: TxMetrics::new(TxDirection::Dst, dst_label.clone())
                .with_counterparty(src_label.clone()),
        };

        let src_tx_worker = SrcTxWorker {
            relay: Arc::clone(self),
            receiver: src_tx_req_rx,
            token: pipeline_token.clone(),
            metrics: TxMetrics::new(TxDirection::Src, src_label.clone())
                .with_counterparty(dst_label.clone()),
        };

        let event_watcher_handle = spawn_worker(event_watcher);
        let packet_worker_handle = spawn_worker(packet_worker);
        let tx_worker_handle = spawn_worker(tx_worker);
        let src_tx_worker_handle = spawn_worker(src_tx_worker);
        let client_refresh_handle = spawn_worker(client_refresh);

        let clearing_handle = config.clearing_interval.map_or_else(
            || tokio::spawn(futures::future::pending()),
            |interval| {
                let packet_sweeper = PacketSweeper {
                    relay: Arc::clone(self),
                    sender: event_tx,
                    token: pipeline_token.clone(),
                    interval,
                    packet_filter: config.packet_filter.clone(),
                    metrics: ClearingMetrics::new(src_label.clone())
                        .with_counterparty(dst_label.clone()),
                };
                spawn_worker(packet_sweeper)
            },
        );

        let misbehaviour_handle = config.misbehaviour_scan_interval.map_or_else(
            || tokio::spawn(futures::future::pending()),
            |interval| {
                let misbehaviour_worker = MisbehaviourWorker {
                    relay: Arc::clone(self),
                    token: pipeline_token.clone(),
                    scan_interval: interval,
                    metrics: MisbehaviourMetrics::new(src_label.clone())
                        .with_counterparty(dst_label.clone())
                        .with_client_id(self.dst_client_id.to_string()),
                };
                spawn_worker(misbehaviour_worker)
            },
        );

        // RAII guards: gauge increments on creation, decrements on drop.
        let _guards = [
            WorkerGuard::with_chain_labels("event_watcher", &src_label, Some(&dst_label)),
            WorkerGuard::with_chain_labels("packet_worker", &src_label, Some(&dst_label)),
            WorkerGuard::with_chain_labels("tx_worker", &dst_label, Some(&src_label)),
            WorkerGuard::with_chain_labels("src_tx_worker", &src_label, Some(&dst_label)),
        ];

        info!("relay pipeline started");

        let result = tokio::select! {
            res = event_watcher_handle => res,
            res = packet_worker_handle => res,
            res = tx_worker_handle => res,
            res = src_tx_worker_handle => res,
        };

        // Tear down remaining workers from this pipeline iteration.
        pipeline_token.cancel();

        let _ = tokio::join!(client_refresh_handle, clearing_handle, misbehaviour_handle);

        match result {
            Ok(worker_result) => worker_result,
            Err(ref join_err) => {
                error!(error = %join_err, "worker task panicked");
                Err(eyre::eyre!("{join_err}"))
            }
        }
    }

    pub async fn run(self: Arc<Self>, config: RelayWorkerConfig) -> Result<()> {
        self.run_with_token(CancellationToken::new(), config).await
    }
}
