use std::sync::Arc;
use std::time::Duration;

use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder,
};
use mercury_chain_traits::events::PacketEvents;
use mercury_chain_traits::inner::Core;
use mercury_chain_traits::queries::{ClientQuery, MisbehaviourQuery};
use mercury_chain_traits::relay::{Relay, RelayChain};
use mercury_chain_traits::types::{ChainTypes, IbcTypes};
use mercury_core::error::Result;
use mercury_core::worker::spawn_worker;
use mercury_telemetry::guard::WorkerGuard;
use mercury_telemetry::recorder::{
    ClientMetrics, EventMetrics, MisbehaviourMetrics, PacketMetrics, SweepMetrics, TxDirection,
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
const MAX_RESTART_BACKOFF: Duration = Duration::from_mins(1);
const INITIAL_RESTART_BACKOFF: Duration = Duration::from_secs(1);

/// Configuration for optional relay workers.
#[derive(Clone, Default)]
pub struct RelayWorkerConfig {
    pub lookback: Option<Duration>,
    pub sweep_interval: Option<Duration>,
    pub misbehaviour_scan_interval: Option<Duration>,
    pub packet_filter: Option<PacketFilter>,
    pub clear_on_start: bool,
    pub clear_limit: usize,
    pub excluded_sequences: Vec<u64>,
}

pub struct RelayContext<Src: ChainTypes, Dst: ChainTypes> {
    pub src_chain: Src,
    pub dst_chain: Dst,
    pub src_client_id: Src::ClientId,
    pub dst_client_id: Dst::ClientId,
}

impl<Src, Dst> Relay for RelayContext<Src, Dst>
where
    Src: RelayChain + ClientPayloadBuilder<Core<Dst>> + PacketEvents,
    Dst: RelayChain
        + ClientMessageBuilder<
            Core<Src>,
            CreateClientPayload = <Src as ClientPayloadBuilder<Core<Dst>>>::CreateClientPayload,
            UpdateClientPayload = <Src as ClientPayloadBuilder<Core<Dst>>>::UpdateClientPayload,
        > + ClientQuery<Core<Src>>
        + PacketMessageBuilder<Core<Src>>
        + ClientPayloadBuilder<Core<Src>>,
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
        + ClientPayloadBuilder<Core<Dst>>
        + PacketEvents
        + PacketMessageBuilder<Core<Dst>>
        + ClientQuery<Core<Dst>>
        + ClientMessageBuilder<
            Core<Dst>,
            CreateClientPayload = <Dst as ClientPayloadBuilder<Core<Src>>>::CreateClientPayload,
            UpdateClientPayload = <Dst as ClientPayloadBuilder<Core<Src>>>::UpdateClientPayload,
        > + MisbehaviourDetector<Core<Dst>, CounterpartyClientState = <Dst as IbcTypes>::ClientState>,
    Dst: RelayChain
        + ClientMessageBuilder<
            Core<Src>,
            CreateClientPayload = <Src as ClientPayloadBuilder<Core<Dst>>>::CreateClientPayload,
            UpdateClientPayload = <Src as ClientPayloadBuilder<Core<Dst>>>::UpdateClientPayload,
        > + ClientQuery<Core<Src>>
        + PacketEvents
        + PacketMessageBuilder<Core<Src>>
        + ClientPayloadBuilder<Core<Src>>
        + MisbehaviourQuery<
            Core<Src>,
            CounterpartyUpdateHeader = <Src as MisbehaviourDetector<Core<Dst>>>::UpdateHeader,
        > + MisbehaviourMessageBuilder<
            Core<Src>,
            MisbehaviourEvidence = <Src as MisbehaviourDetector<Core<Dst>>>::MisbehaviourEvidence,
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

    #[expect(
        clippy::too_many_lines,
        reason = "linear setup flow reads clearer as one function"
    )]
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
            metrics: EventMetrics::new(&src_label).with_counterparty(&dst_label),
        };

        let client_refresh = ClientRefreshWorker {
            relay: Arc::clone(self),
            sender: tx_req_tx.clone(),
            token: pipeline_token.clone(),
            metrics: ClientMetrics::new(&src_label)
                .with_counterparty(&dst_label)
                .with_client_id(self.dst_client_id.to_string()),
            last_upgrade_plan_height: None,
        };

        let packet_worker = PacketWorker {
            relay: Arc::clone(self),
            receiver: event_rx,
            sender: tx_req_tx,
            src_sender: src_tx_req_tx,
            token: pipeline_token.clone(),
            metrics: PacketMetrics::new(&src_label).with_counterparty(&dst_label),
        };

        let tx_worker = TxWorker {
            relay: Arc::clone(self),
            receiver: tx_req_rx,
            token: pipeline_token.clone(),
            metrics: TxMetrics::new(TxDirection::Dst, &dst_label).with_counterparty(&src_label),
        };

        let src_tx_worker = SrcTxWorker {
            relay: Arc::clone(self),
            receiver: src_tx_req_rx,
            token: pipeline_token.clone(),
            metrics: TxMetrics::new(TxDirection::Src, &src_label).with_counterparty(&dst_label),
        };

        let event_watcher_handle = spawn_worker(event_watcher);
        let packet_worker_handle = spawn_worker(packet_worker);
        let tx_worker_handle = spawn_worker(tx_worker);
        let src_tx_worker_handle = spawn_worker(src_tx_worker);
        let client_refresh_handle = spawn_worker(client_refresh);

        let sweep_handle = config.sweep_interval.map(|interval| {
            spawn_worker(PacketSweeper {
                relay: Arc::clone(self),
                sender: event_tx,
                token: pipeline_token.clone(),
                interval,
                packet_filter: config.packet_filter.clone(),
                metrics: SweepMetrics::new(&src_label).with_counterparty(&dst_label),
                clear_on_start: config.clear_on_start,
                clear_limit: config.clear_limit,
                excluded_sequences: config.excluded_sequences.clone(),
            })
        });

        let misbehaviour_handle = config.misbehaviour_scan_interval.map(|interval| {
            spawn_worker(MisbehaviourWorker {
                relay: Arc::clone(self),
                token: pipeline_token.clone(),
                scan_interval: interval,
                metrics: MisbehaviourMetrics::new(&src_label)
                    .with_counterparty(&dst_label)
                    .with_client_id(self.dst_client_id.to_string()),
            })
        });

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

        let optional_handles = sweep_handle.into_iter().chain(misbehaviour_handle);
        let _ = tokio::join!(
            client_refresh_handle,
            futures::future::join_all(optional_handles)
        );

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

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    use super::*;
    use crate::testhelpers::*;

    type MockContext = RelayContext<MockChain, MockChain>;

    fn context(src: MockState, dst: MockState) -> (Arc<MockContext>, SharedState, SharedState) {
        let src = Arc::new(Mutex::new(src));
        let dst = Arc::new(Mutex::new(dst));
        let ctx = RelayContext {
            src_chain: MockChain::new(Arc::clone(&src)),
            dst_chain: MockChain::new(Arc::clone(&dst)),
            src_client_id: "src-client".to_owned(),
            dst_client_id: "dst-client".to_owned(),
        };
        (Arc::new(ctx), src, dst)
    }

    /// Optional sweeper and misbehaviour workers disabled.
    fn config() -> RelayWorkerConfig {
        RelayWorkerConfig::default()
    }

    async fn join_within(handle: tokio::task::JoinHandle<Result<()>>) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(60), handle)
            .await
            .expect("relay did not shut down")
            .unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_pipeline_shuts_down_cleanly() {
        let (ctx, _, _) = context(MockState::default(), MockState::default());
        let token = CancellationToken::new();
        let handle = tokio::spawn(ctx.run_with_token(token.clone(), config()));

        tokio::time::sleep(Duration::from_secs(3)).await;
        token.cancel();
        join_within(handle).await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_pipeline_shuts_down_with_optional_workers_enabled() {
        let (ctx, _, _) = context(MockState::default(), MockState::default());
        let config = RelayWorkerConfig {
            sweep_interval: Some(Duration::from_secs(3600)),
            misbehaviour_scan_interval: Some(Duration::from_secs(3600)),
            ..config()
        };
        let token = CancellationToken::new();
        let handle = tokio::spawn(ctx.run_with_token(token.clone(), config));

        tokio::time::sleep(Duration::from_secs(3)).await;
        token.cancel();
        join_within(handle).await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn failed_worker_restarts_pipeline() {
        // Without lookback the event watcher queries src status itself and fails.
        let src = MockState {
            status_error: true,
            ..Default::default()
        };
        let (ctx, src_state, _) = context(src, MockState::default());
        let token = CancellationToken::new();
        let handle = tokio::spawn(ctx.run_with_token(token.clone(), config()));

        tokio::time::sleep(Duration::from_millis(7500)).await;
        token.cancel();

        assert!(join_within(handle).await.is_err());
        assert!(src_state.lock().unwrap().status_queries >= 4);
    }

    #[tokio::test(start_paused = true)]
    async fn misbehaviour_stops_pipeline_without_restart() {
        let src = MockState {
            check_results: HashMap::from([(5, CheckResult::Evidence)]),
            ..Default::default()
        };
        let dst = MockState {
            consensus_heights: vec![5],
            ..Default::default()
        };
        let (ctx, _, dst_state) = context(src, dst);
        let config = RelayWorkerConfig {
            misbehaviour_scan_interval: Some(Duration::from_secs(10)),
            ..config()
        };

        join_within(tokio::spawn(ctx.run(config))).await.unwrap();

        assert_eq!(dst_state.lock().unwrap().messages_sent.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_pipeline_restarts_with_exponential_backoff() {
        let src = MockState {
            status_error: true,
            ..Default::default()
        };
        let (ctx, src_state, _) = context(src, MockState::default());
        let config = RelayWorkerConfig {
            lookback: Some(Duration::from_secs(60)),
            ..config()
        };
        let token = CancellationToken::new();
        let handle = tokio::spawn(ctx.run_with_token(token.clone(), config));

        // Attempts at t = 0, 1, 3, 7 (backoff 1s, 2s, 4s).
        tokio::time::sleep(Duration::from_millis(7500)).await;
        token.cancel();

        assert!(join_within(handle).await.is_err());
        assert_eq!(src_state.lock().unwrap().status_queries, 4);
    }

    #[tokio::test(start_paused = true)]
    async fn lookback_starts_watcher_in_the_past() {
        let (ctx, src_state, _) = context(MockState::default(), MockState::default());
        let config = RelayWorkerConfig {
            lookback: Some(Duration::from_secs(3)),
            ..config()
        };
        let token = CancellationToken::new();
        let handle = tokio::spawn(ctx.run_with_token(token.clone(), config));

        tokio::time::sleep(Duration::from_millis(1500)).await;
        token.cancel();
        join_within(handle).await.unwrap();

        // 3s lookback at 1s blocks from height 100 starts at 97.
        assert_eq!(src_state.lock().unwrap().queried_blocks, vec![98, 99]);
    }

    #[tokio::test(start_paused = true)]
    async fn swept_packet_flows_through_pipeline_to_dst() {
        let src = MockState {
            commitment_seqs: vec![1],
            committed: HashSet::from([1]),
            send_packets: HashMap::from([(1, MockPacket::new(1))]),
            ..Default::default()
        };
        let (ctx, _, dst_state) = context(src, MockState::default());
        let config = RelayWorkerConfig {
            sweep_interval: Some(Duration::from_secs(3600)),
            clear_on_start: true,
            clear_limit: 10,
            ..config()
        };
        let token = CancellationToken::new();
        let handle = tokio::spawn(ctx.run_with_token(token.clone(), config));

        tokio::time::timeout(Duration::from_secs(60), async {
            while dst_state.lock().unwrap().messages_sent.is_empty() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("recv message never reached dst");
        token.cancel();
        join_within(handle).await.unwrap();

        assert_eq!(dst_state.lock().unwrap().tx_batches, vec![1]);
    }
}
