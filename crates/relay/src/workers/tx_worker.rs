use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::future::BoxFuture;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use mercury_chain_traits::relay::Relay;
use mercury_chain_traits::types::{ChainTypes, MessageSender};
use mercury_core::error::Result;
use mercury_core::worker::Worker;
use mercury_telemetry::recorder::TxMetrics;

use crate::workers::{DstTxRequest, SrcTxRequest, TimestampedMessages};

const MAX_IN_FLIGHT: usize = 3;
const MAX_CONSECUTIVE_FAILURES: usize = 25;
const FORWARD_BUFFER: usize = 256;
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

#[instrument(skip_all, name = "tx_loop", fields(label = label))]
async fn run_tx_loop<M: Send + 'static>(
    label: &'static str,
    metrics: TxMetrics,
    receiver: &mut mpsc::Receiver<TimestampedMessages<M>>,
    token: &CancellationToken,
    send_fn: impl Fn(Vec<M>, Instant) -> BoxFuture<'static, bool> + Send + Sync,
) -> Result<()> {
    let semaphore = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
    let mut join_set = JoinSet::new();
    let mut consecutive_failures: usize = 0;

    loop {
        let first = tokio::select! {
            Some(batch) = receiver.recv() => batch,
            () = token.cancelled() => break,
            Some(result) = join_set.join_next() => {
                match result {
                    Ok(true) => { consecutive_failures = 0; }
                    Ok(false) => {
                        consecutive_failures += 1;
                        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                            warn!(label, consecutive_failures, "consecutive tx failures, cancelling relay");
                            token.cancel();
                            break;
                        }
                        #[expect(clippy::cast_possible_truncation, reason = "value range is bounded in practice")]
                        let backoff = (BACKOFF_BASE * 2u32.saturating_pow(consecutive_failures.min(5) as u32))
                            .min(BACKOFF_CAP);
                        warn!(label, consecutive_failures, max = MAX_CONSECUTIVE_FAILURES, ?backoff, "tx failure, backing off");
                        tokio::select! {
                            () = token.cancelled() => break,
                            () = tokio::time::sleep(backoff) => {}
                        }
                    }
                    Err(e) => { warn!(label, error = %e, "tx task panicked"); }
                }
                metrics.record_consecutive_failures(consecutive_failures);
                continue;
            }
        };

        let mut messages = first.messages;
        let mut oldest_created_at = first.created_at;

        while let Ok(batch) = receiver.try_recv() {
            if batch.created_at < oldest_created_at {
                oldest_created_at = batch.created_at;
            }
            messages.extend(batch.messages);
        }

        let msg_count = messages.len();
        debug!(
            label,
            msg_count, "received messages from channel, submitting"
        );
        let permit = tokio::select! {
            permit = semaphore.clone().acquire_owned() => permit?,
            () = token.cancelled() => {
                warn!(label, msg_count, "dropping batched messages due to cancellation");
                break;
            }
        };

        let fut = send_fn(messages, oldest_created_at);
        join_set.spawn(async move {
            let success = fut.await;
            drop(permit);
            success
        });
    }

    while let Some(result) = join_set.join_next().await {
        if let Err(e) = result {
            warn!(label, error = %e, "tx task panicked during shutdown");
        }
    }

    Ok(())
}

/// Submits batched messages to the destination chain.
pub struct TxWorker<R: Relay> {
    pub relay: Arc<R>,
    pub receiver: mpsc::Receiver<DstTxRequest<R>>,
    pub token: CancellationToken,
    pub metrics: TxMetrics,
}

#[async_trait]
impl<R: Relay> Worker for TxWorker<R> {
    fn name(&self) -> &'static str {
        "tx_worker"
    }

    #[instrument(skip_all, name = "tx_worker", fields(chain = %self.relay.dst_chain().chain_label()))]
    async fn run(mut self) -> Result<()> {
        let metrics = self.metrics.clone();
        let (mut msg_rx, fwd_task) = forward_requests(self.receiver, metrics.clone());
        let relay = self.relay;

        let result = run_tx_loop(
            "dst_tx",
            metrics.clone(),
            &mut msg_rx,
            &self.token,
            move |messages, created_at| {
                let relay = Arc::clone(&relay);
                let metrics = metrics.clone();
                let msg_count = messages.len();
                Box::pin(async move {
                    match relay.dst_chain().send_messages(messages).await {
                        Ok(receipt) => {
                            info!(count = msg_count, "dst batch confirmed");
                            metrics.record_success(
                                msg_count,
                                created_at,
                                receipt.confirmed_at,
                                receipt.gas_used,
                            );
                            true
                        }
                        Err(e) => {
                            warn!(count = msg_count, error = %e, "dst batch failed");
                            metrics.record_error(&e);
                            false
                        }
                    }
                })
            },
        )
        .await;

        fwd_task.abort();
        result
    }
}

/// Submits batched messages to the source chain (e.g. timeouts).
pub struct SrcTxWorker<R: Relay> {
    pub relay: Arc<R>,
    pub receiver: mpsc::Receiver<SrcTxRequest<R>>,
    pub token: CancellationToken,
    pub metrics: TxMetrics,
}

#[async_trait]
impl<R: Relay> Worker for SrcTxWorker<R> {
    fn name(&self) -> &'static str {
        "src_tx_worker"
    }

    #[instrument(skip_all, name = "src_tx_worker", fields(chain = %self.relay.src_chain().chain_label()))]
    async fn run(mut self) -> Result<()> {
        let metrics = self.metrics.clone();
        let (mut msg_rx, fwd_task) = forward_requests(self.receiver, metrics.clone());
        let relay = self.relay;

        let result = run_tx_loop(
            "src_tx",
            metrics.clone(),
            &mut msg_rx,
            &self.token,
            move |messages, created_at| {
                let relay = Arc::clone(&relay);
                let metrics = metrics.clone();
                let msg_count = messages.len();
                Box::pin(async move {
                    match relay.src_chain().send_messages(messages).await {
                        Ok(receipt) => {
                            info!(count = msg_count, "src batch confirmed");
                            metrics.record_success(
                                msg_count,
                                created_at,
                                receipt.confirmed_at,
                                receipt.gas_used,
                            );
                            true
                        }
                        Err(e) => {
                            warn!(count = msg_count, error = %e, "src batch failed");
                            metrics.record_error(&e);
                            false
                        }
                    }
                })
            },
        )
        .await;

        fwd_task.abort();
        result
    }
}

fn forward_requests<T, M>(
    mut req_rx: mpsc::Receiver<T>,
    metrics: TxMetrics,
) -> (
    mpsc::Receiver<TimestampedMessages<M>>,
    tokio::task::JoinHandle<()>,
)
where
    T: Into<TimestampedMessages<M>> + Send + 'static,
    M: Send + 'static,
{
    let (tx, rx) = mpsc::channel(FORWARD_BUFFER);
    let task = tokio::spawn(async move {
        while let Some(req) = req_rx.recv().await {
            let fill = tx.max_capacity() - tx.capacity();
            metrics.record_channel_utilization(fill);
            if tx.send(req.into()).await.is_err() {
                break;
            }
        }
    });
    (rx, task)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::testhelpers::*;

    type Calls = Arc<Mutex<Vec<(Vec<u32>, Instant)>>>;

    fn tx_metrics() -> TxMetrics {
        TxMetrics::new(mercury_telemetry::recorder::TxDirection::Dst, &mock_label())
    }

    fn batch(messages: Vec<u32>, created_at: Instant) -> TimestampedMessages<u32> {
        TimestampedMessages {
            messages,
            created_at,
        }
    }

    struct ScriptedLoop {
        tx: mpsc::Sender<TimestampedMessages<u32>>,
        token: CancellationToken,
        calls: Calls,
        /// Signals each `send_fn` invocation.
        called: mpsc::UnboundedReceiver<()>,
        handle: tokio::task::JoinHandle<Result<()>>,
    }

    impl ScriptedLoop {
        /// Submits one single-message batch and waits until `send_fn` sees it.
        async fn submit(&mut self, msg: usize) {
            self.tx
                .send(batch(vec![u32::try_from(msg).unwrap()], Instant::now()))
                .await
                .unwrap();
            self.called.recv().await.unwrap();
        }
    }

    /// Spawns `run_tx_loop` with a `send_fn` that records each call and
    /// returns the next scripted result (default: success).
    fn spawn_loop(results: Vec<bool>) -> ScriptedLoop {
        let (tx, mut rx) = mpsc::channel(64);
        let token = CancellationToken::new();
        let calls: Calls = Arc::default();
        let (called_tx, called_rx) = mpsc::unbounded_channel();
        let results = Arc::new(Mutex::new(results.into_iter()));

        let loop_token = token.clone();
        let loop_calls = Arc::clone(&calls);
        let handle = tokio::spawn(async move {
            run_tx_loop(
                "test",
                tx_metrics(),
                &mut rx,
                &loop_token,
                move |msgs, at| {
                    loop_calls.lock().unwrap().push((msgs, at));
                    let ok = results.lock().unwrap().next().unwrap_or(true);
                    let _ = called_tx.send(());
                    Box::pin(async move { ok })
                },
            )
            .await
        });
        ScriptedLoop {
            tx,
            token,
            calls,
            called: called_rx,
            handle,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn queued_batches_are_coalesced_with_oldest_timestamp() {
        let (tx, mut rx) = mpsc::channel(8);
        let base = Instant::now();
        let oldest = base.checked_sub(Duration::from_secs(5)).unwrap();
        tx.send(batch(vec![1, 2], base)).await.unwrap();
        tx.send(batch(vec![3], oldest)).await.unwrap();
        tx.send(batch(vec![4], base)).await.unwrap();

        let token = CancellationToken::new();
        let calls: Calls = Arc::default();
        let recorded = Arc::clone(&calls);
        let cancel = token.clone();
        run_tx_loop("test", tx_metrics(), &mut rx, &token, move |msgs, at| {
            recorded.lock().unwrap().push((msgs, at));
            cancel.cancel();
            Box::pin(async { true })
        })
        .await
        .unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, vec![1, 2, 3, 4]);
        assert_eq!(calls[0].1, oldest);
    }

    #[tokio::test(start_paused = true)]
    async fn consecutive_failures_cancel_relay_after_backoff() {
        let mut lp = spawn_loop(vec![false; MAX_CONSECUTIVE_FAILURES]);
        let start = tokio::time::Instant::now();

        for i in 0..MAX_CONSECUTIVE_FAILURES {
            lp.submit(i).await;
        }
        lp.handle.await.unwrap().unwrap();

        assert!(lp.token.is_cancelled());
        assert_eq!(lp.calls.lock().unwrap().len(), MAX_CONSECUTIVE_FAILURES);
        // Backoff 2s, 4s, 8s, 16s, then capped at 30s for failures 5..=24.
        assert_eq!(
            start.elapsed(),
            Duration::from_secs(2 + 4 + 8 + 16 + 20 * 30)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn success_resets_failure_streak() {
        let mut script = vec![false; MAX_CONSECUTIVE_FAILURES - 1];
        script.push(true);
        script.extend(vec![false; MAX_CONSECUTIVE_FAILURES]);
        let mut lp = spawn_loop(script);

        for i in 0..2 * MAX_CONSECUTIVE_FAILURES - 1 {
            lp.submit(i).await;
        }
        tokio::time::sleep(Duration::from_hours(1)).await;
        assert!(
            !lp.token.is_cancelled(),
            "streak should have reset on success"
        );

        lp.submit(0).await;
        lp.handle.await.unwrap().unwrap();
        assert!(lp.token.is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_interrupts_failure_backoff() {
        let mut lp = spawn_loop(vec![false]);
        lp.submit(0).await;
        // Let the loop observe the failure and enter its 2s backoff.
        tokio::time::sleep(Duration::from_millis(10)).await;

        lp.token.cancel();
        tokio::time::timeout(Duration::from_secs(1), lp.handle)
            .await
            .expect("backoff ignored cancellation")
            .unwrap()
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_waits_for_in_flight_sends() {
        let (tx, mut rx) = mpsc::channel(8);
        tx.send(batch(vec![1], Instant::now())).await.unwrap();

        let token = CancellationToken::new();
        let done = Arc::new(Mutex::new(false));
        let done_flag = Arc::clone(&done);
        let cancel = token.clone();
        run_tx_loop("test", tx_metrics(), &mut rx, &token, move |_, _| {
            cancel.cancel();
            let done_flag = Arc::clone(&done_flag);
            Box::pin(async move {
                tokio::time::sleep(Duration::from_secs(10)).await;
                *done_flag.lock().unwrap() = true;
                true
            })
        })
        .await
        .unwrap();

        assert!(*done.lock().unwrap());
    }

    async fn wait_for_batches(state: &SharedState, n: usize) {
        while state.lock().unwrap().tx_batches.len() < n {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn dst_worker_submits_to_dst_chain_and_survives_errors() {
        let dst = MockState {
            send_failures: 1,
            ..Default::default()
        };
        let (relay, src_state, dst_state) = mock_relay(MockState::default(), dst);
        let (req_tx, receiver) = mpsc::channel(8);
        let worker = TxWorker {
            relay,
            receiver,
            token: CancellationToken::new(),
            metrics: tx_metrics(),
        };
        let token = worker.token.clone();
        let handle = tokio::spawn(worker.run());

        req_tx
            .send(DstTxRequest {
                messages: vec![MockMsg],
                created_at: Instant::now(),
            })
            .await
            .unwrap();
        wait_for_batches(&dst_state, 1).await;
        req_tx
            .send(DstTxRequest {
                messages: vec![MockMsg, MockMsg],
                created_at: Instant::now(),
            })
            .await
            .unwrap();
        wait_for_batches(&dst_state, 2).await;

        token.cancel();
        handle.await.unwrap().unwrap();

        let dst = dst_state.lock().unwrap();
        assert_eq!(dst.tx_batches, vec![1, 2]);
        assert_eq!(dst.messages_sent.len(), 2, "only the second batch landed");
        assert_eq!(src_state.lock().unwrap().tx_batches, Vec::<usize>::new());
    }

    #[tokio::test(start_paused = true)]
    async fn src_worker_submits_to_src_chain() {
        let src = MockState {
            send_failures: 1,
            ..Default::default()
        };
        let (relay, src_state, dst_state) = mock_relay(src, MockState::default());
        let (req_tx, receiver) = mpsc::channel(8);
        let worker = SrcTxWorker {
            relay,
            receiver,
            token: CancellationToken::new(),
            metrics: tx_metrics(),
        };
        let token = worker.token.clone();
        let handle = tokio::spawn(worker.run());

        for n in 1..=2 {
            req_tx
                .send(SrcTxRequest {
                    messages: vec![MockMsg],
                    created_at: Instant::now(),
                })
                .await
                .unwrap();
            wait_for_batches(&src_state, n).await;
        }

        token.cancel();
        handle.await.unwrap().unwrap();

        assert_eq!(src_state.lock().unwrap().messages_sent.len(), 1);
        assert_eq!(dst_state.lock().unwrap().tx_batches, Vec::<usize>::new());
    }
}
