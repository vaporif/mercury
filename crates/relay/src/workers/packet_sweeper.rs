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

/// Periodically sweeps for stuck packets and acks, re-injecting them into the relay pipeline.
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

        // Reserve at least 25% of the limit for acks so recv-heavy states don't
        // permanently starve ack sweeping.
        let min_ack_budget = self.clear_limit / 4;
        let ack_budget = self
            .clear_limit
            .saturating_sub(recv_unrelayed.len())
            .max(min_ack_budget);

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

        self.metrics
            .record_swept(recv_events.len() + ack_events.len());
        self.metrics.record_recv_cleared(recv_events.len());
        self.metrics.record_ack_cleared(ack_events.len());
        self.metrics.record_excluded(total_excluded);
        self.metrics.record_scan_duration(start.elapsed());

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

/// Find committed packets on src that dst hasn't received yet.
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

    if let SweepScope::Sequences(seqs) = scope {
        commitment_seqs.retain(|s| seqs.contains(&s.0));
    }

    let pre_exclude = commitment_seqs.len();
    commitment_seqs.retain(|s| !excluded_sequences.contains(&s.0));
    let excluded_count = pre_exclude - commitment_seqs.len();

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

/// Find acks on src whose corresponding commitment on dst hasn't been cleared.
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

    if let SweepScope::Sequences(seqs) = scope {
        ack_seqs.retain(|s| seqs.contains(&s.0));
    }

    let pre_exclude = ack_seqs.len();
    ack_seqs.retain(|s| !excluded_sequences.contains(&s.0));
    let excluded_count = pre_exclude - ack_seqs.len();

    ack_seqs.truncate(limit);

    if ack_seqs.is_empty() {
        return Ok((vec![], excluded_count));
    }

    let dst_client_id = relay.dst_client_id().clone();

    let unrelayed: Vec<PacketSequence> = stream::iter(ack_seqs)
        .map(|seq| {
            let dst_client_id = dst_client_id.clone();
            let dst_height = dst_height.clone();
            async move {
                match dst
                    .query_packet_commitment(&dst_client_id, seq, &dst_height)
                    .await
                {
                    Ok((Some(_), _)) => Some(seq),
                    Ok((None, _)) => None,
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

/// Quick scan used by the CLI `clear` command — just counts what's stuck.
pub async fn clear_packets_once<R>(relay: Arc<R>, scope: SweepScope) -> Result<ClearResult>
where
    R: Relay,
{
    let excluded: &[u64] = &[];
    let limit = usize::MAX;

    let (recv_unrelayed, _) =
        discover_unreceived_packets(relay.as_ref(), &scope, excluded, limit).await?;

    let (ack_unrelayed, _) =
        discover_unrelayed_acks(relay.as_ref(), &scope, excluded, limit).await?;

    Ok(ClearResult {
        recv_cleared: recv_unrelayed.len(),
        ack_cleared: ack_unrelayed.len(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use mercury_chain_traits::types::Port;

    use super::*;
    use crate::filter::{FilterPolicy, PacketFilterConfig};
    use crate::testhelpers::*;

    fn sorted(seqs: Vec<PacketSequence>) -> Vec<u64> {
        let mut v: Vec<u64> = seqs.into_iter().map(|s| s.0).collect();
        v.sort_unstable();
        v
    }

    fn sweeper(
        relay: Arc<MockRelay>,
        clear_limit: usize,
    ) -> (
        PacketSweeper<MockRelay>,
        mpsc::Receiver<Vec<IbcEvent<MockRelay>>>,
    ) {
        let (sender, receiver) = mpsc::channel(8);
        let sweeper = PacketSweeper {
            relay,
            sender,
            token: CancellationToken::new(),
            interval: Duration::from_secs(10),
            packet_filter: None,
            metrics: SweepMetrics::new(&mock_label()),
            clear_on_start: false,
            clear_limit,
            excluded_sequences: vec![],
        };
        (sweeper, receiver)
    }

    fn packets(seqs: &[u64]) -> HashMap<u64, MockPacket> {
        seqs.iter().map(|&s| (s, MockPacket::new(s))).collect()
    }

    #[tokio::test]
    async fn unreceived_skips_received_failing_and_excluded() {
        let src = MockState {
            commitment_seqs: vec![1, 2, 3, 4],
            ..Default::default()
        };
        let dst = MockState {
            received: HashSet::from([2]),
            failing_seqs: HashSet::from([3]),
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, dst);
        let (seqs, excluded) =
            discover_unreceived_packets(relay.as_ref(), &SweepScope::All, &[4], usize::MAX)
                .await
                .unwrap();
        assert_eq!(sorted(seqs), vec![1]);
        assert_eq!(excluded, 1);
    }

    #[tokio::test]
    async fn unreceived_respects_scope_and_limit() {
        let src = MockState {
            commitment_seqs: vec![1, 2, 3, 4, 5],
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, MockState::default());

        let scope = SweepScope::Sequences(vec![2, 4, 5]);
        let (seqs, _) = discover_unreceived_packets(relay.as_ref(), &scope, &[], 2)
            .await
            .unwrap();
        assert_eq!(sorted(seqs), vec![2, 4]);
    }

    #[tokio::test]
    async fn unreceived_all_excluded_returns_empty_with_count() {
        let src = MockState {
            commitment_seqs: vec![7, 8],
            ..Default::default()
        };
        let (relay, _, dst) = mock_relay(src, MockState::default());
        let (seqs, excluded) =
            discover_unreceived_packets(relay.as_ref(), &SweepScope::All, &[7, 8], 10)
                .await
                .unwrap();
        assert_eq!(seqs, vec![]);
        assert_eq!(excluded, 2);
        // Only the latest-height lookup, no receipt queries were needed.
        assert_eq!(dst.lock().unwrap().status_queries, 1);
    }

    #[tokio::test]
    async fn unreceived_propagates_query_errors() {
        let src = MockState {
            sequence_query_error: true,
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, MockState::default());
        assert!(
            discover_unreceived_packets(relay.as_ref(), &SweepScope::All, &[], 10)
                .await
                .is_err()
        );

        let dst = MockState {
            status_error: true,
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(MockState::default(), dst);
        assert!(
            discover_unreceived_packets(relay.as_ref(), &SweepScope::All, &[], 10)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn unrelayed_acks_only_returns_seqs_still_committed_on_dst() {
        let src = MockState {
            ack_seqs: vec![1, 2, 3, 4, 5],
            ..Default::default()
        };
        let dst = MockState {
            committed: HashSet::from([1, 2, 3, 5]),
            failing_seqs: HashSet::from([2]),
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, dst);
        let scope = SweepScope::Sequences(vec![1, 2, 3, 4]);
        let (seqs, excluded) = discover_unrelayed_acks(relay.as_ref(), &scope, &[3], 10)
            .await
            .unwrap();
        assert_eq!(sorted(seqs), vec![1]);
        assert_eq!(excluded, 1);
    }

    #[tokio::test]
    async fn unrelayed_acks_propagates_query_errors() {
        let src = MockState {
            sequence_query_error: true,
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, MockState::default());
        assert!(
            discover_unrelayed_acks(relay.as_ref(), &SweepScope::All, &[], 10)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn clear_packets_once_counts_both_directions() {
        let src = MockState {
            commitment_seqs: vec![1, 2, 3],
            ack_seqs: vec![10, 11],
            ..Default::default()
        };
        let dst = MockState {
            received: HashSet::from([1]),
            committed: HashSet::from([10, 11]),
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, dst);
        let result = clear_packets_once(relay, SweepScope::All).await.unwrap();
        assert_eq!((result.recv_cleared, result.ack_cleared), (2, 2));
    }

    #[tokio::test]
    async fn scan_forwards_recovered_send_and_ack_events() {
        let src = MockState {
            commitment_seqs: vec![1, 2, 3],
            ack_seqs: vec![10, 11],
            send_packets: packets(&[1, 3]),
            write_acks: packets(&[10, 11]),
            event_lookup_errors: HashSet::from([3, 11]),
            ..Default::default()
        };
        let dst = MockState {
            committed: HashSet::from([10, 11]),
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, dst);
        let (sweeper, mut rx) = sweeper(relay, 100);

        sweeper.scan(&SweepScope::All).await.unwrap();

        // 2 has no send event (pruned); 3 and 11 fail lookup.
        let (sends, acks) = event_seqs(rx.try_recv().unwrap());
        assert_eq!((sends, acks), (vec![1], vec![10]));
        assert!(!sweeper.token.is_cancelled());
    }

    #[tokio::test]
    async fn scan_applies_packet_filter_to_swept_sends() {
        let src = MockState {
            commitment_seqs: vec![1, 2],
            send_packets: HashMap::from([
                (1, MockPacket::on_port(1, "transfer")),
                (2, MockPacket::on_port(2, "icahost")),
            ]),
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, MockState::default());
        let (mut sweeper, mut rx) = sweeper(relay, 100);
        sweeper.packet_filter = Some(
            PacketFilter::new(&PacketFilterConfig {
                policy: FilterPolicy::Allow,
                source_ports: vec![Port("transfer".into())],
            })
            .unwrap(),
        );

        sweeper.scan(&SweepScope::All).await.unwrap();

        assert_eq!(event_seqs(rx.try_recv().unwrap()), (vec![1], vec![]));
    }

    #[tokio::test]
    async fn scan_reserves_quarter_of_limit_for_acks() {
        let src = MockState {
            commitment_seqs: (1..=8).collect(),
            ack_seqs: (10..=20).collect(),
            send_packets: packets(&(1..=8).collect::<Vec<_>>()),
            write_acks: packets(&(10..=20).collect::<Vec<_>>()),
            ..Default::default()
        };
        let dst = MockState {
            committed: (10..=20).collect(),
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, dst);
        let (sweeper, mut rx) = sweeper(relay, 8);

        sweeper.scan(&SweepScope::All).await.unwrap();

        // Recv consumes the whole limit, but 8/4 = 2 acks are still swept.
        let (sends, acks) = event_seqs(rx.try_recv().unwrap());
        assert_eq!(sends.len(), 8);
        assert_eq!(acks, vec![10, 11]);
    }

    #[tokio::test]
    async fn scan_with_zero_limit_sends_nothing() {
        let src = MockState {
            commitment_seqs: vec![1],
            ack_seqs: vec![2],
            send_packets: packets(&[1]),
            write_acks: packets(&[2]),
            ..Default::default()
        };
        let dst = MockState {
            committed: HashSet::from([2]),
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, dst);
        let (sweeper, mut rx) = sweeper(relay, 0);

        sweeper.scan(&SweepScope::All).await.unwrap();

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn scan_cancels_token_when_channel_closed() {
        let src = MockState {
            commitment_seqs: vec![1],
            send_packets: packets(&[1]),
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, MockState::default());
        let (sweeper, rx) = sweeper(relay, 10);
        drop(rx);

        sweeper.scan(&SweepScope::All).await.unwrap();

        assert!(sweeper.token.is_cancelled());
    }

    #[tokio::test]
    async fn scan_propagates_discovery_error() {
        let src = MockState {
            sequence_query_error: true,
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, MockState::default());
        let (sweeper, _rx) = sweeper(relay, 10);
        assert!(sweeper.scan(&SweepScope::All).await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn run_clears_on_start_then_sweeps_each_interval() {
        let src = MockState {
            commitment_seqs: vec![1],
            send_packets: packets(&[1]),
            ..Default::default()
        };
        let (relay, _, _) = mock_relay(src, MockState::default());
        let (mut sweeper, mut rx) = sweeper(relay, 10);
        sweeper.clear_on_start = true;
        let token = sweeper.token.clone();

        let handle = tokio::spawn(sweeper.run());

        let start = tokio::time::Instant::now();
        assert_eq!(event_seqs(recv(&mut rx).await).0, vec![1]);
        assert_eq!(start.elapsed(), Duration::ZERO);

        assert_eq!(event_seqs(recv(&mut rx).await).0, vec![1]);
        assert_eq!(start.elapsed(), Duration::from_secs(10));

        token.cancel();
        handle.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn run_keeps_going_after_failed_scans() {
        let src = MockState {
            sequence_query_error: true,
            ..Default::default()
        };
        let (relay, src_state, _) = mock_relay(src, MockState::default());
        let (mut sweeper, _rx) = sweeper(relay, 10);
        sweeper.clear_on_start = true;
        let token = sweeper.token.clone();

        let handle = tokio::spawn(sweeper.run());
        tokio::time::sleep(Duration::from_secs(25)).await;
        token.cancel();
        handle.await.unwrap().unwrap();

        // clear-on-start + 2 interval scans, each querying src height once.
        assert_eq!(src_state.lock().unwrap().status_queries, 3);
    }
}
