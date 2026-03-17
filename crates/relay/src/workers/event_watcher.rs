use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, trace, warn};

use mercury_chain_traits::events::PacketEvents;
use mercury_chain_traits::queries::ChainStatusQuery;
use mercury_chain_traits::relay::{IbcEvent, Relay};
use mercury_chain_traits::types::{ChainTypes, IbcTypes};
use mercury_core::error::Result;
use mercury_core::worker::Worker;
use mercury_telemetry::recorder::EventMetrics;

use crate::filter::PacketFilter;

const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Polls the source chain for new blocks and extracts IBC packet events.
pub struct EventWatcher<R: Relay> {
    pub relay: Arc<R>,
    pub sender: mpsc::Sender<Vec<IbcEvent<R>>>,
    pub token: CancellationToken,
    pub start_height: Option<<R::SrcChain as ChainTypes>::Height>,
    pub packet_filter: Option<PacketFilter>,
    pub metrics: EventMetrics,
}

#[async_trait]
impl<R: Relay> Worker for EventWatcher<R> {
    fn name(&self) -> &'static str {
        "event_watcher"
    }

    #[instrument(skip_all, name = "event_watcher", fields(src_chain = %self.relay.src_chain().chain_id()))]
    async fn run(self) -> Result<()> {
        let src = self.relay.src_chain();
        let mut last_height = match self.start_height {
            Some(h) => h,
            None => src.query_latest_height().await?,
        };
        let mut last_block_at = Instant::now();
        info!(start_height = %last_height, "event watcher started");

        loop {
            tokio::select! {
                () = self.token.cancelled() => break,
                () = tokio::time::sleep(POLL_INTERVAL) => {}
            }

            self.metrics.record_lag(last_block_at);

            let Ok(latest) = src
                .query_latest_height()
                .await
                .inspect_err(|e| warn!(error = %e, "failed to query latest height, will retry"))
            else {
                continue;
            };
            if latest <= last_height {
                trace!(height = %last_height, "no new blocks");
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
                        <R::SrcChain as PacketEvents>::try_extract_send_packet_event(event)
                    {
                        ibc_events.push(IbcEvent::SendPacket(send));
                    } else if let Some(write_ack) =
                        <R::SrcChain as PacketEvents>::try_extract_write_ack_event(event)
                    {
                        ibc_events.push(IbcEvent::WriteAck(write_ack));
                    }
                }

                let pre_filter_count = ibc_events.len();
                let send_count = ibc_events
                    .iter()
                    .filter(|e| matches!(e, IbcEvent::SendPacket(_)))
                    .count();
                let ack_count = pre_filter_count - send_count;

                self.metrics.record_send_events(send_count);
                self.metrics.record_ack_events(ack_count);

                if let Some(ref filter) = self.packet_filter {
                    ibc_events.retain(|event| {
                        let packet = match event {
                            IbcEvent::SendPacket(e) => {
                                <R::SrcChain as PacketEvents>::packet_from_send_event(e)
                            }
                            IbcEvent::WriteAck(e) => {
                                <R::SrcChain as PacketEvents>::packet_from_write_ack_event(e).0
                            }
                        };
                        let ports = <R::SrcChain as IbcTypes>::packet_source_ports(packet);
                        let allowed = filter.allows(&ports);
                        if !allowed {
                            let seq = <R::SrcChain as IbcTypes>::packet_sequence(packet);
                            debug!(seq, ?ports, "packet filtered out");
                        }
                        allowed
                    });

                    let filtered_count = pre_filter_count - ibc_events.len();
                    self.metrics.record_filtered(filtered_count);
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
                last_block_at = Instant::now();
                maybe_h = R::SrcChain::increment_height(&h);
            }
        }

        Ok(())
    }
}
