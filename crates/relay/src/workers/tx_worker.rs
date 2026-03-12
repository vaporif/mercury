use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use mercury_chain_traits::messaging::CanSendMessages;
use mercury_chain_traits::relay::context::Relay;
use mercury_core::error::{Error, Result};
use mercury_core::worker::Worker;

use crate::workers::TxRequest;

const MAX_IN_FLIGHT: usize = 3;

pub struct TxWorker<R: Relay> {
    pub relay: Arc<R>,
    pub receiver: mpsc::Receiver<TxRequest<R>>,
    pub token: CancellationToken,
}

#[async_trait]
impl<R: Relay> Worker for TxWorker<R> {
    fn name(&self) -> &'static str {
        "tx_worker"
    }

    async fn run(mut self) -> Result<()> {
        let semaphore = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
        let mut join_set = JoinSet::new();

        loop {
            let first = tokio::select! {
                Some(request) = self.receiver.recv() => request,
                () = self.token.cancelled() => break,
                Some(result) = join_set.join_next() => {
                    if let Err(e) = result {
                        warn!(error = %e, "tx task panicked");
                    }
                    continue;
                }
            };

            let mut messages = first.messages;
            while let Ok(request) = self.receiver.try_recv() {
                messages.extend(request.messages);
            }

            let msg_count = messages.len();
            let permit = tokio::select! {
                permit = semaphore.clone().acquire_owned() => permit.map_err(Error::report)?,
                () = self.token.cancelled() => {
                    warn!(count = msg_count, "dropping batched messages due to cancellation");
                    break;
                }
            };

            let relay = Arc::clone(&self.relay);
            join_set.spawn(async move {
                let result = relay.dst_chain().send_messages(messages).await;
                match &result {
                    Ok(_) => info!(count = msg_count, "batch confirmed"),
                    Err(e) => warn!(count = msg_count, error = %e, "batch failed"),
                }
                // Holds the semaphore permit until send_messages completes
                drop(permit);
            });
        }

        while let Some(result) = join_set.join_next().await {
            if let Err(e) = result {
                warn!(error = %e, "tx task panicked during shutdown");
            }
        }

        Ok(())
    }
}
