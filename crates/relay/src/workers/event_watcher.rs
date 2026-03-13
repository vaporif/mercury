use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, warn};

use mercury_chain_traits::prelude::*;
use mercury_chain_traits::relay::{IbcEvent, Relay};
use mercury_core::error::Result;
use mercury_core::worker::Worker;

const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Polls the source chain for new blocks and extracts IBC packet events.
pub struct EventWatcher<R: Relay> {
    pub relay: Arc<R>,
    pub sender: mpsc::Sender<Vec<IbcEvent<R>>>,
    pub token: CancellationToken,
}

#[async_trait]
impl<R: Relay> Worker for EventWatcher<R> {
    fn name(&self) -> &'static str {
        "event_watcher"
    }

    #[instrument(skip_all, name = "event_watcher")]
    async fn run(self) -> Result<()> {
        let src = self.relay.src_chain();
        let mut last_height = src.query_latest_height().await?;

        loop {
            tokio::select! {
                () = self.token.cancelled() => break,
                () = tokio::time::sleep(POLL_INTERVAL) => {}
            }

            let latest = match src.query_latest_height().await {
                Ok(h) => h,
                Err(e) => {
                    warn!(error = %e, "failed to query latest height, will retry");
                    continue;
                }
            };
            if latest <= last_height {
                continue;
            }

            let mut maybe_h = R::SrcChain::increment_height(&last_height);
            while let Some(h) = maybe_h {
                // Stay 1 block behind tip so proof queries at (height - 1) see the commitment.
                if h >= latest {
                    break;
                }

                let block_events = match src.query_block_events(&h).await {
                    Ok(events) => events,
                    Err(e) => {
                        warn!(height = %h, error = %e, "failed to query block events, will retry from this height");
                        break;
                    }
                };

                debug!(height = %h, event_count = block_events.len(), "polled block events");

                let mut ibc_events = Vec::new();
                for event in &block_events {
                    if let Some(send) =
                        <R::SrcChain as PacketEvents<R::DstChain>>::try_extract_send_packet_event(
                            event,
                        )
                    {
                        ibc_events.push(IbcEvent::SendPacket(send));
                    } else if let Some(write_ack) =
                        <R::SrcChain as PacketEvents<R::DstChain>>::try_extract_write_ack_event(
                            event,
                        )
                    {
                        ibc_events.push(IbcEvent::WriteAck(write_ack));
                    }
                }

                if !ibc_events.is_empty() {
                    debug!(height = %h, count = ibc_events.len(), "extracted IBC events");
                    if self.sender.send(ibc_events).await.is_err() {
                        warn!("packet_worker channel closed, cancelling relay");
                        self.token.cancel();
                        return Ok(());
                    }
                }

                last_height = h.clone();
                maybe_h = R::SrcChain::increment_height(&h);
            }
        }

        Ok(())
    }
}
