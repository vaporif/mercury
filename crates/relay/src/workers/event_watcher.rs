use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use mercury_chain_traits::events::{CanExtractPacketEvents, CanQueryBlockEvents};
use mercury_chain_traits::relay::context::Relay;
use mercury_chain_traits::relay::ibc_event::IbcEvent;
use mercury_core::error::Result;
use mercury_core::worker::Worker;

const POLL_INTERVAL: Duration = Duration::from_secs(1);

pub struct EventWatcher<R: Relay> {
    pub relay: Arc<R>,
    pub sender: mpsc::Sender<Vec<IbcEvent<R>>>,
    pub token: CancellationToken,
}

#[async_trait]
impl<R: Relay> Worker for EventWatcher<R>
where
    R::SrcChain: CanQueryBlockEvents,
{
    fn name(&self) -> &'static str {
        "event_watcher"
    }

    async fn run(self) -> Result<()> {
        let src = self.relay.src_chain();
        let mut last_height = src.query_latest_height().await?;

        loop {
            tokio::select! {
                () = self.token.cancelled() => break,
                () = tokio::time::sleep(POLL_INTERVAL) => {}
            }

            let latest = src.query_latest_height().await?;
            if latest <= last_height {
                continue;
            }

            let mut maybe_h = R::SrcChain::increment_height(&last_height);
            while let Some(h) = maybe_h {
                if h > latest {
                    break;
                }

                let block_events = src.query_block_events(&h).await?;

                let mut ibc_events = Vec::new();
                for event in &block_events {
                    if let Some(send) =
                        <R::SrcChain as CanExtractPacketEvents<R::DstChain>>::try_extract_send_packet_event(event)
                    {
                        ibc_events.push(IbcEvent::SendPacket(send));
                    } else if let Some(write_ack) =
                        <R::SrcChain as CanExtractPacketEvents<R::DstChain>>::try_extract_write_ack_event(event)
                    {
                        ibc_events.push(IbcEvent::WriteAck(write_ack));
                    }
                }

                if !ibc_events.is_empty() {
                    debug!(height = %h, count = ibc_events.len(), "extracted IBC events");
                    if self.sender.send(ibc_events).await.is_err() {
                        warn!("packet_worker channel closed");
                        break;
                    }
                }

                maybe_h = R::SrcChain::increment_height(&h);
            }

            last_height = latest;
        }

        Ok(())
    }
}
