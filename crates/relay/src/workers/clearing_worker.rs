use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use mercury_chain_traits::events::PacketEvents;
use mercury_chain_traits::queries::{ChainStatusQuery, PacketStateQuery};
use mercury_chain_traits::relay::{IbcEvent, Relay};
use mercury_chain_traits::types::IbcTypes;
use mercury_core::error::Result;
use mercury_core::worker::Worker;

use mercury_telemetry::recorder::ClearingMetrics;

use crate::filter::PacketFilter;

const RECEIPT_CHECK_CONCURRENCY: usize = 8;

/// Scans for unrelayed packet commitments and feeds recovered events into the relay pipeline.
pub struct ClearingWorker<R: Relay> {
    pub relay: Arc<R>,
    pub sender: mpsc::Sender<Vec<IbcEvent<R>>>,
    pub token: CancellationToken,
    pub interval: Duration,
    pub packet_filter: Option<PacketFilter>,
    pub metrics: ClearingMetrics,
}

#[async_trait]
impl<R: Relay> Worker for ClearingWorker<R> {
    fn name(&self) -> &'static str {
        "clearing_worker"
    }

    #[instrument(skip_all, name = "clearing_worker")]
    async fn run(self) -> Result<()> {
        loop {
            if let Err(e) = self.scan().await {
                warn!(error = %e, "clearing scan failed, will retry next interval");
            }

            tokio::select! {
                () = self.token.cancelled() => break,
                () = tokio::time::sleep(self.interval) => {}
            }
        }

        Ok(())
    }
}

impl<R: Relay> ClearingWorker<R> {
    async fn scan(&self) -> Result<()> {
        let src = self.relay.src_chain();
        let dst = self.relay.dst_chain();

        let src_height = src.query_latest_height().await?;
        let dst_height = dst.query_latest_height().await?;

        let commitment_seqs = src
            .query_commitment_sequences(self.relay.src_client_id(), &src_height)
            .await?;

        let total = commitment_seqs.len();
        if total == 0 {
            debug!("clearing scan complete: no commitments found");
            return Ok(());
        }

        let dst_client_id = self.relay.dst_client_id().clone();

        let unrelayed: Vec<u64> = stream::iter(commitment_seqs)
            .map(|seq| {
                let dst_client_id = dst_client_id.clone();
                let dst_height = dst_height.clone();
                async move {
                    match dst
                        .query_packet_receipt(&dst_client_id, seq, &dst_height)
                        .await
                    {
                        Ok((Some(_), _)) => None,
                        Ok((None, _)) => Some(seq),
                        Err(e) => {
                            warn!(seq, error = %e, "failed to query receipt, skipping sequence");
                            None
                        }
                    }
                }
            })
            .buffer_unordered(RECEIPT_CHECK_CONCURRENCY)
            .filter_map(|x| async move { x })
            .collect()
            .await;

        if unrelayed.is_empty() {
            info!(found = total, unrelayed = 0, "clearing scan complete");
            return Ok(());
        }

        let src_client_id = self.relay.src_client_id().clone();
        let mut events: Vec<IbcEvent<R>> = Vec::new();
        for seq in &unrelayed {
            match src.query_send_packet_event(&src_client_id, *seq).await {
                Ok(Some(send_event)) => {
                    if let Some(ref filter) = self.packet_filter {
                        let packet =
                            <R::SrcChain as PacketEvents>::packet_from_send_event(&send_event);
                        let ports = <R::SrcChain as IbcTypes>::packet_source_ports(packet);
                        if !filter.allows(&ports) {
                            debug!(seq, ?ports, "cleared packet filtered out");
                            continue;
                        }
                    }
                    events.push(IbcEvent::SendPacket(send_event));
                }
                Ok(None) => {
                    warn!(seq, "send packet event not found, may have been pruned");
                }
                Err(e) => {
                    warn!(seq, error = %e, "failed to recover send packet event, skipping");
                }
            }
        }

        self.metrics.record_cleared(events.len());

        if !events.is_empty() && self.sender.send(events).await.is_err() {
            warn!("packet_worker channel closed, cancelling relay");
            self.token.cancel();
            return Ok(());
        }

        info!(
            found = total,
            unrelayed = unrelayed.len(),
            "clearing scan complete"
        );
        Ok(())
    }
}
