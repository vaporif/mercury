use std::sync::Arc;
use std::time::Duration;

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

use crate::workers::{DstTxRequest, SrcTxRequest};

const MAX_IN_FLIGHT: usize = 3;
const MAX_CONSECUTIVE_FAILURES: usize = 25;
const FORWARD_BUFFER: usize = 256;
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

#[instrument(skip_all, name = "tx_loop", fields(label = label))]
async fn run_tx_loop<M: Send + 'static>(
    label: &'static str,
    receiver: &mut mpsc::Receiver<Vec<M>>,
    token: &CancellationToken,
    send_fn: impl Fn(Vec<M>) -> BoxFuture<'static, bool> + Send + Sync,
) -> Result<()> {
    let semaphore = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
    let mut join_set = JoinSet::new();
    let mut consecutive_failures: usize = 0;

    loop {
        let first = tokio::select! {
            Some(messages) = receiver.recv() => messages,
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
                continue;
            }
        };

        let mut messages = first;
        while let Ok(batch) = receiver.try_recv() {
            messages.extend(batch);
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

        let fut = send_fn(messages);
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
}

#[async_trait]
impl<R: Relay> Worker for TxWorker<R> {
    fn name(&self) -> &'static str {
        "tx_worker"
    }

    #[instrument(skip_all, name = "tx_worker")]
    async fn run(mut self) -> Result<()> {
        let (mut msg_rx, fwd_task) = forward_requests(self.receiver);
        let relay = self.relay;

        let result = run_tx_loop("dst_tx", &mut msg_rx, &self.token, move |messages| {
            let relay = Arc::clone(&relay);
            let msg_count = messages.len();
            Box::pin(async move {
                match relay.dst_chain().send_messages(messages).await {
                    Ok(_) => {
                        info!(count = msg_count, "dst batch confirmed");
                        true
                    }
                    Err(e) => {
                        warn!(count = msg_count, error = %e, "dst batch failed");
                        false
                    }
                }
            })
        })
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
}

#[async_trait]
impl<R: Relay> Worker for SrcTxWorker<R> {
    fn name(&self) -> &'static str {
        "src_tx_worker"
    }

    #[instrument(skip_all, name = "src_tx_worker")]
    async fn run(mut self) -> Result<()> {
        let (mut msg_rx, fwd_task) = forward_requests(self.receiver);
        let relay = self.relay;

        let result = run_tx_loop("src_tx", &mut msg_rx, &self.token, move |messages| {
            let relay = Arc::clone(&relay);
            let msg_count = messages.len();
            Box::pin(async move {
                match relay.src_chain().send_messages(messages).await {
                    Ok(_) => {
                        info!(count = msg_count, "src batch confirmed");
                        true
                    }
                    Err(e) => {
                        warn!(count = msg_count, error = %e, "src batch failed");
                        false
                    }
                }
            })
        })
        .await;

        fwd_task.abort();
        result
    }
}

fn forward_requests<T, M>(
    mut req_rx: mpsc::Receiver<T>,
) -> (mpsc::Receiver<Vec<M>>, tokio::task::JoinHandle<()>)
where
    T: Into<Vec<M>> + Send + 'static,
    M: Send + 'static,
{
    let (tx, rx) = mpsc::channel(FORWARD_BUFFER);
    let task = tokio::spawn(async move {
        while let Some(req) = req_rx.recv().await {
            if tx.send(req.into()).await.is_err() {
                break;
            }
        }
    });
    (rx, task)
}
