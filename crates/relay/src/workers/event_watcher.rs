use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use mercury_chain_traits::events::{BlockEventStream, PacketEvents};
use mercury_chain_traits::queries::ChainStatusQuery;
use mercury_chain_traits::relay::{IbcEvent, Relay};
use mercury_chain_traits::types::{ChainTypes, IbcTypes};
use mercury_core::error::Result;
use mercury_core::worker::Worker;
use mercury_telemetry::recorder::EventMetrics;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, trace, warn};

use crate::filter::PacketFilter;

const POLL_INTERVAL: Duration = Duration::from_secs(1);

type SrcBlockEventStream<R> = BlockEventStream<
    <<R as Relay>::SrcChain as ChainTypes>::Height,
    <<R as Relay>::SrcChain as ChainTypes>::Event,
>;

/// Watches the source chain for IBC packet events via WebSocket push or RPC polling fallback.
pub struct EventWatcher<R: Relay> {
    pub relay: Arc<R>,
    pub sender: mpsc::Sender<Vec<IbcEvent<R>>>,
    pub token: CancellationToken,
    pub start_height: Option<<R::SrcChain as ChainTypes>::Height>,
    pub packet_filter: Option<PacketFilter>,
    pub metrics: EventMetrics,
}

impl<R: Relay> EventWatcher<R> {
    fn extract_and_filter(
        &self,
        block_events: &[<R::SrcChain as ChainTypes>::Event],
    ) -> Vec<IbcEvent<R>> {
        let mut ibc_events = Vec::new();
        for event in block_events {
            if let Some(send) = <R::SrcChain as PacketEvents>::try_extract_send_packet_event(event)
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
                    debug!(%seq, ?ports, "packet filtered out");
                }
                allowed
            });

            let filtered_count = pre_filter_count - ibc_events.len();
            self.metrics.record_filtered(filtered_count);
        }

        ibc_events
    }

    async fn send_events(&self, ibc_events: Vec<IbcEvent<R>>) -> bool {
        if !ibc_events.is_empty() && self.sender.send(ibc_events).await.is_err() {
            warn!("packet_worker channel closed, cancelling relay");
            self.token.cancel();
            return false;
        }
        true
    }

    fn fallback_to_polling(
        &self,
        ws_stream: &mut Option<SrcBlockEventStream<R>>,
        reconnect_rx: &mut Option<oneshot::Receiver<SrcBlockEventStream<R>>>,
        reconnect_cancel: &CancellationToken,
    ) {
        self.metrics.record_ws_fallback();
        self.metrics.record_event_source_mode(false);
        *ws_stream = None;
        *reconnect_rx = Some(Self::spawn_reconnect(
            &self.relay,
            reconnect_cancel,
            &self.metrics,
        ));
    }

    async fn background_reconnect(
        relay: Arc<R>,
        tx: oneshot::Sender<SrcBlockEventStream<R>>,
        cancel: CancellationToken,
        metrics: EventMetrics,
    ) {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(300);
        let mut attempt = 0u64;

        loop {
            if cancel.is_cancelled() {
                return;
            }

            attempt += 1;
            metrics.record_ws_reconnect_attempt();

            match relay.src_chain().subscribe_block_events().await {
                Ok(Some(stream)) => {
                    let _ = tx.send(stream);
                    return;
                }
                Ok(None) => {
                    return;
                }
                Err(e) => {
                    metrics.record_ws_reconnect_failed();
                    warn!(
                        error = %e,
                        attempt,
                        next_backoff_secs = backoff.as_secs(),
                        "websocket reconnect failed"
                    );
                }
            }

            tokio::select! {
                () = cancel.cancelled() => return,
                () = tokio::time::sleep(backoff) => {}
            }

            backoff = (backoff * 2).min(max_backoff);
        }
    }

    fn spawn_reconnect(
        relay: &Arc<R>,
        reconnect_cancel: &CancellationToken,
        metrics: &EventMetrics,
    ) -> oneshot::Receiver<SrcBlockEventStream<R>> {
        let relay = Arc::clone(relay);
        let cancel = reconnect_cancel.child_token();
        let (tx, rx) = oneshot::channel();
        let metrics = metrics.clone();
        tokio::spawn(async move {
            Self::background_reconnect(relay, tx, cancel, metrics).await;
        });
        rx
    }
}

#[async_trait]
impl<R: Relay> Worker for EventWatcher<R> {
    fn name(&self) -> &'static str {
        "event_watcher"
    }

    #[instrument(skip_all, name = "event_watcher", fields(src_chain = %self.relay.src_chain().chain_label()))]
    async fn run(self) -> Result<()> {
        let src = self.relay.src_chain();
        let mut last_height = match self.start_height {
            Some(ref h) => h.clone(),
            None => src.query_latest_height().await?,
        };
        let mut last_block_at = Instant::now();
        info!(start_height = %last_height, "event watcher started");

        let mut initial_ws_failed = false;
        let mut ws_stream: Option<SrcBlockEventStream<R>> = match src.subscribe_block_events().await
        {
            Ok(Some(stream)) => {
                info!("websocket connected, using push-based event source");
                self.metrics.record_event_source_mode(true);
                self.metrics.record_ws_connected();
                Some(stream)
            }
            Ok(None) => {
                info!("no websocket configured, using rpc polling");
                self.metrics.record_event_source_mode(false);
                None
            }
            Err(e) => {
                warn!(error = %e, "websocket connect failed, falling back to rpc polling");
                self.metrics.record_event_source_mode(false);
                self.metrics.record_ws_fallback();
                initial_ws_failed = true;
                None
            }
        };

        let mut reconnect_rx: Option<oneshot::Receiver<SrcBlockEventStream<R>>> = None;
        let reconnect_cancel = CancellationToken::new();

        if initial_ws_failed {
            reconnect_rx = Some(Self::spawn_reconnect(
                &self.relay,
                &reconnect_cancel,
                &self.metrics,
            ));
        }

        let result = loop {
            if let Some(ref mut stream) = ws_stream {
                self.metrics.record_lag(last_block_at);

                tokio::select! {
                    () = self.token.cancelled() => break Ok(()),
                    item = stream.next() => {
                        match item {
                            Some(Ok(block)) => {
                                debug!(height = %block.height, event_count = block.events.len(), "ws block events");
                                self.metrics.record_ws_events(block.events.len());

                                let ibc_events = self.extract_and_filter(&block.events);
                                if !self.send_events(ibc_events).await {
                                    break Ok(());
                                }
                                last_height = block.height;
                                last_block_at = Instant::now();
                                self.metrics.record_lag(last_block_at);
                            }
                            Some(Err(e)) => {
                                warn!(error = %e, "websocket stream error, falling back to rpc polling");
                                self.fallback_to_polling(&mut ws_stream, &mut reconnect_rx, &reconnect_cancel);
                            }
                            None => {
                                warn!("websocket stream ended, falling back to rpc polling");
                                self.fallback_to_polling(&mut ws_stream, &mut reconnect_rx, &reconnect_cancel);
                            }
                        }
                    }
                }
            } else {
                if let Some(ref mut rx) = reconnect_rx {
                    match rx.try_recv() {
                        Ok(new_stream) => {
                            info!(
                                "websocket reconnected, switching back to push-based event source"
                            );
                            self.metrics.record_event_source_mode(true);
                            self.metrics.record_ws_connected();
                            ws_stream = Some(new_stream);
                            reconnect_rx = None;
                            continue;
                        }
                        Err(oneshot::error::TryRecvError::Empty) => {}

                        Err(oneshot::error::TryRecvError::Closed) => {
                            reconnect_rx = None;
                        }
                    }
                }

                tokio::select! {
                    () = self.token.cancelled() => break Ok(()),
                    () = tokio::time::sleep(POLL_INTERVAL) => {}
                }

                self.metrics.record_lag(last_block_at);

                let Ok(latest) = src.query_latest_height().await.inspect_err(
                    |e| warn!(error = %e, "failed to query latest height, will retry"),
                ) else {
                    continue;
                };
                if latest <= last_height {
                    trace!(height = %last_height, "no new blocks");
                    continue;
                }

                let mut maybe_h = R::SrcChain::increment_height(&last_height);
                while let Some(h) = maybe_h {
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

                    let ibc_events = self.extract_and_filter(&block_events);
                    if !self.send_events(ibc_events).await {
                        break;
                    }

                    last_height = h.clone();
                    last_block_at = Instant::now();
                    maybe_h = R::SrcChain::increment_height(&h);
                }
            }
        };

        reconnect_cancel.cancel();

        result
    }
}
