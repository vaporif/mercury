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
use mercury_chain_traits::types::{ChainTypes, IbcTypes, PacketSequence};
use mercury_core::error::Result;
use mercury_core::plugin::{ClearResult, SweepScope};
use mercury_core::worker::Worker;

use mercury_telemetry::recorder::SweepMetrics;

use crate::filter::PacketFilter;

const RECEIPT_CHECK_CONCURRENCY: usize = 8;

/// Scans for unrelayed packet commitments and acks, feeding recovered events into the relay pipeline.
pub struct PacketSweeper<R: Relay> {
    pub relay: Arc<R>,
    pub sender: mpsc::Sender<Vec<IbcEvent<R>>>,
    pub token: CancellationToken,
    pub interval: Duration,
    pub packet_filter: Option<PacketFilter>,
    pub metrics: SweepMetrics,
    pub clear_on_start: bool,
    pub clear_limit: usize,
    pub excluded_sequences: Vec<u64>,
}

#[async_trait]
impl<R: Relay> Worker for PacketSweeper<R> {
    fn name(&self) -> &'static str {
        "packet_sweeper"
    }

    #[instrument(skip_all, name = "packet_sweeper", fields(src_chain = %self.relay.src_chain().chain_label(), dst_chain = %self.relay.dst_chain().chain_label()))]
    async fn run(mut self) -> Result<()> {
        if self.clear_on_start {
            info!("running clear-on-start sweep");
            if let Err(e) = self.scan(&SweepScope::All).await {
                warn!(error = %e, "clear-on-start sweep failed");
            }
            self.clear_on_start = false;
        }

        loop {
            tokio::select! {
                () = self.token.cancelled() => break,
                () = tokio::time::sleep(self.interval) => {}
            }

            if let Err(e) = self.scan(&SweepScope::All).await {
                warn!(error = %e, "sweep scan failed, will retry next interval");
            }
        }

        Ok(())
    }
}

impl<R: Relay> PacketSweeper<R> {
    async fn scan(&self, scope: &SweepScope) -> Result<()> {
        let start = std::time::Instant::now();
        debug!("starting sweep scan");

        let mut total_excluded = 0;

        // Phase 1: Recv clearing
        let (recv_unrelayed, recv_excluded) = discover_unreceived_packets(
            self.relay.as_ref(),
            scope,
            &self.excluded_sequences,
            self.clear_limit,
        )
        .await
        .inspect_err(|_| self.metrics.record_error("recv"))?;

        total_excluded += recv_excluded;

        let src_client_id = self.relay.src_client_id().clone();
        let src = self.relay.src_chain();

        let mut recv_events: Vec<IbcEvent<R>> = Vec::new();
        for seq in &recv_unrelayed {
            match src.query_send_packet_event(&src_client_id, *seq).await {
                Ok(Some(send_event)) => {
                    if let Some(ref filter) = self.packet_filter {
                        let packet =
                            <R::SrcChain as PacketEvents>::packet_from_send_event(&send_event);
                        let ports = <R::SrcChain as IbcTypes>::packet_source_ports(packet);
                        if !filter.allows(&ports) {
                            debug!(%seq, ?ports, "swept packet filtered out");
                            continue;
                        }
                    }
                    recv_events.push(IbcEvent::SendPacket(send_event));
                }
                Ok(None) => {
                    warn!(%seq, "send packet event not found, may have been pruned");
                }
                Err(e) => {
                    warn!(%seq, error = %e, "failed to recover send packet event, skipping");
                }
            }
        }

        // Phase 2: Ack clearing (uses remaining budget)
        let ack_budget = self.clear_limit.saturating_sub(recv_unrelayed.len());

        let (ack_unrelayed, ack_excluded) = if ack_budget > 0 {
            discover_unrelayed_acks(
                self.relay.as_ref(),
                scope,
                &self.excluded_sequences,
                ack_budget,
            )
            .await
            .inspect_err(|_| self.metrics.record_error("ack"))?
        } else {
            (vec![], 0)
        };

        total_excluded += ack_excluded;

        let mut ack_events: Vec<IbcEvent<R>> = Vec::new();
        for seq in &ack_unrelayed {
            match src.query_write_ack_event(&src_client_id, *seq).await {
                Ok(Some(write_ack)) => {
                    ack_events.push(IbcEvent::WriteAck(write_ack));
                }
                Ok(None) => {
                    warn!(%seq, "write ack event not found, may have been pruned");
                }
                Err(e) => {
                    warn!(%seq, error = %e, "failed to recover write ack event, skipping");
                }
            }
        }

        // Record metrics
        self.metrics
            .record_swept(recv_events.len() + ack_events.len());
        self.metrics.record_recv_cleared(recv_events.len());
        self.metrics.record_ack_cleared(ack_events.len());
        self.metrics.record_excluded(total_excluded);
        self.metrics.record_scan_duration(start.elapsed());

        // Send events to pipeline
        let mut all_events = recv_events;
        all_events.extend(ack_events);

        if !all_events.is_empty() && self.sender.send(all_events).await.is_err() {
            warn!("packet_worker channel closed, cancelling relay");
            self.token.cancel();
            return Ok(());
        }

        info!(
            recv = recv_unrelayed.len(),
            ack = ack_unrelayed.len(),
            excluded = total_excluded,
            "sweep complete"
        );
        Ok(())
    }
}

/// Discover unreceived packets: commitments on src with no receipt on dst.
/// Returns `(unrelayed_sequences, excluded_count)`.
pub async fn discover_unreceived_packets<R: Relay>(
    relay: &R,
    scope: &SweepScope,
    excluded_sequences: &[u64],
    limit: usize,
) -> Result<(Vec<PacketSequence>, usize)> {
    let src = relay.src_chain();
    let dst = relay.dst_chain();
    let src_height = src.query_latest_height().await?;
    let dst_height = dst.query_latest_height().await?;

    let mut commitment_seqs = src
        .query_commitment_sequences(relay.src_client_id(), &src_height)
        .await?;

    // Apply scope filter
    if let SweepScope::Sequences(seqs) = scope {
        commitment_seqs.retain(|s| seqs.contains(&s.0));
    }

    // Apply excluded_sequences filter
    let pre_exclude = commitment_seqs.len();
    commitment_seqs.retain(|s| !excluded_sequences.contains(&s.0));
    let excluded_count = pre_exclude - commitment_seqs.len();

    // Apply limit
    commitment_seqs.truncate(limit);

    if commitment_seqs.is_empty() {
        return Ok((vec![], excluded_count));
    }

    let dst_client_id = relay.dst_client_id().clone();

    let unrelayed: Vec<PacketSequence> = stream::iter(commitment_seqs)
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
                        warn!(%seq, error = %e, "failed to query receipt, skipping sequence");
                        None
                    }
                }
            }
        })
        .buffer_unordered(RECEIPT_CHECK_CONCURRENCY)
        .filter_map(|x| async move { x })
        .collect()
        .await;

    Ok((unrelayed, excluded_count))
}

/// Discover unrelayed acks: acks written on src, but commitment still exists on dst.
/// Returns `(unrelayed_sequences, excluded_count)`.
pub async fn discover_unrelayed_acks<R: Relay>(
    relay: &R,
    scope: &SweepScope,
    excluded_sequences: &[u64],
    limit: usize,
) -> Result<(Vec<PacketSequence>, usize)> {
    let src = relay.src_chain();
    let dst = relay.dst_chain();
    let src_height = src.query_latest_height().await?;
    let dst_height = dst.query_latest_height().await?;

    let mut ack_seqs = src
        .query_ack_sequences(relay.src_client_id(), &src_height)
        .await?;

    // Apply scope filter
    if let SweepScope::Sequences(seqs) = scope {
        ack_seqs.retain(|s| seqs.contains(&s.0));
    }

    // Apply excluded_sequences filter
    let pre_exclude = ack_seqs.len();
    ack_seqs.retain(|s| !excluded_sequences.contains(&s.0));
    let excluded_count = pre_exclude - ack_seqs.len();

    // Apply limit
    ack_seqs.truncate(limit);

    if ack_seqs.is_empty() {
        return Ok((vec![], excluded_count));
    }

    let dst_client_id = relay.dst_client_id().clone();

    // For ack clearing: if dst still has a commitment, the ack hasn't been relayed
    let unrelayed: Vec<PacketSequence> = stream::iter(ack_seqs)
        .map(|seq| {
            let dst_client_id = dst_client_id.clone();
            let dst_height = dst_height.clone();
            async move {
                match dst
                    .query_packet_commitment(&dst_client_id, seq, &dst_height)
                    .await
                {
                    Ok((Some(_), _)) => Some(seq), // commitment exists = ack not relayed
                    Ok((None, _)) => None,         // no commitment = already acked
                    Err(e) => {
                        warn!(%seq, error = %e, "failed to query commitment for ack, skipping");
                        None
                    }
                }
            }
        })
        .buffer_unordered(RECEIPT_CHECK_CONCURRENCY)
        .filter_map(|x| async move { x })
        .collect()
        .await;

    Ok((unrelayed, excluded_count))
}

/// One-shot packet discovery: find unrelayed packets and acks, return counts.
///
/// Shares discovery logic with `PacketSweeper` but only reports what was found.
/// Does not build or submit relay messages — the counts indicate how many
/// unrelayed items exist. Useful for CLI diagnostics.
// TODO: extend to build and submit messages directly for full CLI clearing.
pub async fn clear_packets_once<R>(relay: Arc<R>, scope: SweepScope) -> Result<ClearResult>
where
    R: Relay,
{
    let excluded: &[u64] = &[];
    let limit = usize::MAX;

    // Phase 1: Recv
    let (recv_unrelayed, _) =
        discover_unreceived_packets(relay.as_ref(), &scope, excluded, limit).await?;

    let src_client_id = relay.src_client_id().clone();
    let src = relay.src_chain();

    let mut recv_count = 0;
    for seq in &recv_unrelayed {
        match src.query_send_packet_event(&src_client_id, *seq).await {
            Ok(Some(_)) => recv_count += 1,
            Ok(None) => warn!(%seq, "send packet event not found"),
            Err(e) => warn!(%seq, error = %e, "failed to recover send packet"),
        }
    }

    // Phase 2: Ack
    let (ack_unrelayed, _) =
        discover_unrelayed_acks(relay.as_ref(), &scope, excluded, limit).await?;

    let mut ack_count = 0;
    for seq in &ack_unrelayed {
        match src.query_write_ack_event(&src_client_id, *seq).await {
            Ok(Some(_)) => ack_count += 1,
            Ok(None) => warn!(%seq, "write ack event not found"),
            Err(e) => warn!(%seq, error = %e, "failed to recover write ack"),
        }
    }

    Ok(ClearResult {
        recv_cleared: recv_count,
        ack_cleared: ack_count,
    })
}
