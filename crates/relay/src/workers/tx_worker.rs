use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::future::BoxFuture;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use mercury_chain_traits::relay::Relay;
use mercury_chain_traits::types::MessageSender;
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
                            warn!("{label}: {consecutive_failures} consecutive tx failures, cancelling relay");
                            token.cancel();
                            break;
                        }
                        let backoff = (BACKOFF_BASE * 2u32.saturating_pow(consecutive_failures.min(5).try_into().unwrap_or(5)))
                            .min(BACKOFF_CAP);
                        warn!("{label}: tx failure {consecutive_failures}/{MAX_CONSECUTIVE_FAILURES}, backing off {backoff:?}");
                        tokio::time::sleep(backoff).await;
                    }
                    Err(e) => { warn!("{label}: tx task panicked: {e}"); }
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
        debug!("{label}: received {msg_count} messages from channel, submitting");
        let permit = tokio::select! {
            permit = semaphore.clone().acquire_owned() => permit?,
            () = token.cancelled() => {
                warn!("{label}: dropping {msg_count} batched messages due to cancellation");
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
            warn!("{label}: tx task panicked during shutdown: {e}");
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

    #[instrument(skip_all, name = "tx_worker")]
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

    #[instrument(skip_all, name = "src_tx_worker")]
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
