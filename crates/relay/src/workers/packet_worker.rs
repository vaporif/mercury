use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, UpdateClientOutput,
};
use mercury_chain_traits::events::PacketEvents;
use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use mercury_chain_traits::relay::{DstCore, IbcEvent, Relay, RelayPacketBuilder, SrcCore};
use mercury_chain_traits::types::{ChainTypes, IbcTypes};
use mercury_core::error::Result;
use mercury_core::worker::Worker;
use mercury_telemetry::recorder::PacketMetrics;

use crate::workers::{DstTxRequest, SrcTxRequest};

/// `(proof_height, message_proof_height, update messages)`; see
/// `build_dst_update_client_payload` for the two heights.
type UpdateClientResult<Height, Message> = Result<(Height, Option<Height>, Vec<Message>)>;
/// `(messages, built, unbuilt, membership entries)`.
type BuildMessagesResult<Message, E> = (
    Vec<Message>,
    Vec<E>,
    Vec<E>,
    Vec<mercury_core::MembershipProofEntry>,
);

const PROOF_FETCH_CONCURRENCY: usize = 8;
const PROOF_FETCH_MAX_RETRIES: usize = 3;
const PROOF_FETCH_RETRY_DELAY: Duration = Duration::from_millis(500);

const PENDING_RECHECK_INTERVAL: Duration = Duration::from_secs(5);
const RECV_GRACE_PERIOD: Duration = Duration::from_secs(15);

async fn send_or_cancel<T>(
    sender: &mpsc::Sender<T>,
    request: T,
    token: &CancellationToken,
    label: &str,
) -> bool {
    if sender.send(request).await.is_err() {
        warn!("{label} channel closed, cancelling relay");
        token.cancel();
        return true;
    }
    false
}

/// Turns IBC events into relay messages (recv, ack, timeout).
pub struct PacketWorker<R: Relay> {
    pub relay: Arc<R>,
    pub receiver: mpsc::Receiver<Vec<IbcEvent<R>>>,
    pub sender: mpsc::Sender<DstTxRequest<R>>,
    pub src_sender: mpsc::Sender<SrcTxRequest<R>>,
    pub token: CancellationToken,
    pub metrics: PacketMetrics,
}

struct PendingSend<E> {
    event: E,
    recv_sent_at: Option<Instant>,
}

impl<R> PacketWorker<R>
where
    R: Relay + RelayPacketBuilder,
    R::SrcChain: ClientQuery<DstCore<R>>
        + ClientMessageBuilder<
            DstCore<R>,
            CreateClientPayload = <R::DstChain as ClientPayloadBuilder<SrcCore<R>>>::CreateClientPayload,
            UpdateClientPayload = <R::DstChain as ClientPayloadBuilder<SrcCore<R>>>::UpdateClientPayload,
        >,
{
    /// Builds update-client headers (no proofs yet).
    ///
    /// Returns `(proof_height, message_proof_height, payload)`:
    /// - `proof_height` -- src height for proof queries (execution block for beacon)
    /// - `message_proof_height` -- dst IBC message height (slot for beacon), `None` reuses `proof_height`
    /// - `payload` -- headers, `None` if already up to date
    #[instrument(skip_all, name = "build_dst_update_client", fields(src_chain = %self.relay.src_chain().chain_label(), dst_chain = %self.relay.dst_chain().chain_label()))]
    #[expect(clippy::type_complexity, reason = "return type mirrors chain associated types")]
    async fn build_dst_update_client_payload(
        &self,
    ) -> Result<(
        SrcHeight<R>,
        Option<SrcHeight<R>>,
        Option<<R::DstChain as ClientMessageBuilder<SrcCore<R>>>::UpdateClientPayload>,
    )>{
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        let src_status = self.relay.src_chain().query_chain_status().await?;
        let src_height = SrcChain::<R>::chain_status_height(&src_status).clone();

        let dst_status = self.relay.dst_chain().query_chain_status().await?;
        let dst_height = DstChain::<R>::chain_status_height(&dst_status).clone();

        let client_state = self
            .relay
            .dst_chain()
            .query_client_state(self.relay.dst_client_id(), &dst_height)
            .await?;
        let trusted_height = DstChain::<R>::client_latest_height(&client_state);

        if src_height <= trusted_height {
            let proof_height = self
                .relay
                .src_chain()
                .proof_height_without_update(&src_height, &trusted_height);
            return Ok((proof_height, None, None));
        }

        debug!(
            %trusted_height,
            %src_height,
            "building update client payload for dst"
        );
        let update_payload = self
            .relay
            .src_chain()
            .build_update_client_payload(&trusted_height, &src_height, &client_state)
            .await?;

        if let Some(required_ts) = self.relay.src_chain().required_dst_timestamp_secs(&update_payload) {
            super::wait_for_dst_timestamp::<R>(self.relay.dst_chain(), required_ts).await?;
        }

        let proof_height = self
            .relay
            .src_chain()
            .update_payload_proof_height(&update_payload)
            .unwrap_or(src_height);

        let message_proof_height = self
            .relay
            .src_chain()
            .update_payload_message_height(&update_payload);

        Ok((proof_height, message_proof_height, Some(update_payload)))
    }

    async fn build_dst_update_client_message(
        &self,
        mut payload: <R::DstChain as ClientMessageBuilder<SrcCore<R>>>::UpdateClientPayload,
        membership_entries: &[mercury_core::MembershipProofEntry],
    ) -> Result<UpdateClientOutput<<R::DstChain as ChainTypes>::Message>> {
        if !membership_entries.is_empty() {
            info!(
                count = membership_entries.len(),
                "enriching update payload with membership proofs"
            );
            self.relay
                .dst_chain()
                .enrich_update_payload(&mut payload, membership_entries);
        }

        self.relay
            .dst_chain()
            .build_update_client_message(self.relay.dst_client_id(), payload)
            .await
    }

    #[instrument(skip_all, name = "build_src_update_client", fields(src_chain = %self.relay.src_chain().chain_label(), dst_chain = %self.relay.dst_chain().chain_label()))]
    async fn build_src_update_client_messages(
        &self,
    ) -> UpdateClientResult<DstHeight<R>, <R::SrcChain as ChainTypes>::Message>
    {
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        let dst_status = self.relay.dst_chain().query_chain_status().await?;
        let dst_height = DstChain::<R>::chain_status_height(&dst_status).clone();

        let src_status = self.relay.src_chain().query_chain_status().await?;
        let src_height = SrcChain::<R>::chain_status_height(&src_status).clone();

        let client_state = self
            .relay
            .src_chain()
            .query_client_state(self.relay.src_client_id(), &src_height)
            .await?;
        let trusted_height = SrcChain::<R>::client_latest_height(&client_state);

        if dst_height <= trusted_height {
            let proof_height = self
                .relay
                .dst_chain()
                .proof_height_without_update(&dst_height, &trusted_height);
            return Ok((proof_height, None, vec![]));
        }

        let update_payload = self
            .relay
            .dst_chain()
            .build_update_client_payload(&trusted_height, &dst_height, &client_state)
            .await?;
        let proof_height = self
            .relay
            .dst_chain()
            .update_payload_proof_height(&update_payload)
            .unwrap_or(dst_height);
        let message_proof_height = self
            .relay
            .dst_chain()
            .update_payload_message_height(&update_payload);
        let update_output = self
            .relay
            .src_chain()
            .build_update_client_message(self.relay.src_client_id(), update_payload)
            .await?;

        Ok((proof_height, message_proof_height, update_output.messages))
    }

    #[instrument(skip_all, name = "build_recv_tracked", fields(src_chain = %self.relay.src_chain().chain_label(), dst_chain = %self.relay.dst_chain().chain_label()))]
    async fn build_recv_messages_tracked(
        &self,
        send_packets: Vec<PendingSend<SendEvent<R>>>,
        src_height: &SrcHeight<R>,
        message_proof_height: Option<&SrcHeight<R>>,
    ) -> BuildMessagesResult<DstMessage<R>, PendingSend<SendEvent<R>>> {
        type SrcChain<R> = <R as Relay>::SrcChain;

        let relay = &self.relay;

        let results: Vec<_> = stream::iter(send_packets)
            .map(|ps| {
                let relay = relay.clone();
                let src_height = src_height.clone();
                let msg_height = message_proof_height.cloned();
                async move {
                    let pkt = <SrcChain<R> as PacketEvents>::packet_from_send_event(&ps.event);
                    let result = retry_proof_fetch(|| async {
                        relay
                            .build_receive_packet_messages(
                                pkt,
                                &src_height,
                                msg_height.as_ref(),
                            )
                            .await
                    })
                    .await;
                    (ps, result)
                }
            })
            .buffered(PROOF_FETCH_CONCURRENCY)
            .collect()
            .await;

        let mut messages = Vec::new();
        let mut built = Vec::new();
        let mut unbuilt = Vec::new();
        let mut membership_entries = Vec::new();

        for (ps, result) in results {
            match result {
                Ok((msgs, _entries)) if msgs.is_empty() => {
                    unbuilt.push(ps);
                }
                Ok((msgs, entries)) => {
                    messages.extend(msgs);
                    membership_entries.extend(entries);
                    built.push(ps);
                }
                Err(e) => {
                    warn!(error = %e, "recv proof fetch failed after retries, will retry");
                    unbuilt.push(ps);
                }
            }
        }

        (messages, built, unbuilt, membership_entries)
    }

    fn record_backlog_metrics(
        &self,
        in_flight: &[PendingSend<SendEvent<R>>],
        deliverable: &[PendingSend<SendEvent<R>>],
        timed_out: &[PendingSend<SendEvent<R>>],
    ) {
        type SrcChain<R> = <R as Relay>::SrcChain;
        let total = in_flight.len() + deliverable.len() + timed_out.len();
        self.metrics.record_backlog(total);

        let oldest = in_flight
            .iter()
            .chain(deliverable.iter())
            .chain(timed_out.iter())
            .map(|ps| {
                let pkt = <SrcChain<R> as PacketEvents>::packet_from_send_event(&ps.event);
                u64::from(<SrcChain<R> as IbcTypes>::packet_sequence(pkt))
            })
            .min();

        if let Some(seq) = oldest {
            self.metrics.record_oldest_sequence(seq);
        }
    }

    /// Checks dst receipts for in-flight packets past the grace period.
    /// Got a receipt? Drop it. No receipt? Reset and retry.
    #[instrument(skip_all, name = "confirm_in_flight", fields(src_chain = %self.relay.src_chain().chain_label(), dst_chain = %self.relay.dst_chain().chain_label()))]
    async fn confirm_in_flight(
        &self,
        in_flight: Vec<PendingSend<SendEvent<R>>>,
        dst_height: &DstHeight<R>,
    ) -> Vec<PendingSend<SendEvent<R>>> {
        type SrcChain<R> = <R as Relay>::SrcChain;

        let relay = &self.relay;

        let results: Vec<_> = stream::iter(in_flight)
            .map(|ps| {
                let relay = relay.clone();
                let dst_height = dst_height.clone();
                async move {
                    let pkt = <SrcChain<R> as PacketEvents>::packet_from_send_event(&ps.event);
                    let seq = <SrcChain<R> as IbcTypes>::packet_sequence(pkt);
                    let result = relay
                        .dst_chain()
                        .query_packet_receipt(relay.dst_client_id(), seq, &dst_height)
                        .await;
                    (ps, seq, result)
                }
            })
            .buffered(PROOF_FETCH_CONCURRENCY)
            .collect()
            .await;

        let mut still_pending = Vec::new();
        for (ps, seq, result) in results {
            match result {
                Ok((receipt, _proof)) if receipt.is_some() => {
                    debug!(%seq, "receipt exists on dst, packet delivered");
                }
                Ok(_) => {
                    still_pending.push(PendingSend {
                        event: ps.event,
                        recv_sent_at: None,
                    });
                }
                Err(e) => {
                    debug!(%seq, error = %e, "receipt query failed, keeping pending");
                    still_pending.push(ps);
                }
            }
        }

        still_pending
    }

    #[instrument(skip_all, name = "build_ack_tracked", fields(src_chain = %self.relay.src_chain().chain_label(), dst_chain = %self.relay.dst_chain().chain_label()))]
    async fn build_ack_messages_tracked(
        &self,
        write_acks: WriteAckEvents<R>,
        src_height: &SrcHeight<R>,
        message_proof_height: Option<&SrcHeight<R>>,
    ) -> BuildMessagesResult<DstMessage<R>, WriteAckEvent<R>> {
        type SrcChain<R> = <R as Relay>::SrcChain;

        let relay = &self.relay;

        let results: Vec<_> = stream::iter(write_acks)
            .map(|e| {
                let relay = relay.clone();
                let src_height = src_height.clone();
                let msg_height = message_proof_height.cloned();
                async move {
                    let (pkt, ack) = <SrcChain<R> as PacketEvents>::packet_from_write_ack_event(&e);
                    let result = retry_proof_fetch(|| async {
                        relay
                            .build_ack_packet_messages(
                                pkt,
                                ack,
                                &src_height,
                                msg_height.as_ref(),
                            )
                            .await
                    })
                    .await;
                    (e, result)
                }
            })
            .buffered(PROOF_FETCH_CONCURRENCY)
            .collect()
            .await;

        let mut messages = Vec::new();
        let mut built = Vec::new();
        let mut failed = Vec::new();
        let mut membership_entries = Vec::new();

        for (event, result) in results {
            match result {
                Ok((msgs, _entries)) if msgs.is_empty() => {
                    failed.push(event);
                }
                Ok((msgs, entries)) => {
                    messages.extend(msgs);
                    membership_entries.extend(entries);
                    built.push(event);
                }
                Err(e) => {
                    warn!(error = %e, "ack proof fetch failed after retries, will retry");
                    failed.push(event);
                }
            }
        }

        (messages, built, failed, membership_entries)
    }

    /// Combines the client update with packet messages into one dst batch.
    /// `None` means the batch was given up and its packets must be retried.
    async fn build_dst_batch(
        &self,
        maybe_payload: Option<<R::DstChain as ClientMessageBuilder<SrcCore<R>>>::UpdateClientPayload>,
        mut packet_msgs: Vec<DstMessage<R>>,
        membership_entries: &[mercury_core::MembershipProofEntry],
    ) -> Option<Vec<DstMessage<R>>> {
        let Some(payload) = maybe_payload else {
            if !packet_msgs.is_empty() {
                info!(
                    count = packet_msgs.len(),
                    "client up to date, sending packet messages only"
                );
            }
            return Some(packet_msgs);
        };

        if packet_msgs.is_empty() {
            return self
                .build_dst_update_client_message(payload, &[])
                .await
                .inspect_err(|e| warn!(error = %e, "failed to build update client message, skipping"))
                .ok()
                .map(|output| output.messages);
        }

        debug!(
            membership_entries = membership_entries.len(),
            packet_msgs = packet_msgs.len(),
            "building update client message with membership proofs"
        );

        let mut update_output = self
            .build_dst_update_client_message(payload, membership_entries)
            .await
            .inspect_err(|e| warn!(error = %e, "failed to build update client message with proofs, skipping batch"))
            .ok()?;
        let update_msg_count = update_output.messages.len();

        debug!(
            update_msgs = update_msg_count,
            packet_msgs = packet_msgs.len(),
            has_membership_proof = update_output.membership_proof.is_some(),
            "pre-finalize message counts"
        );

        if update_msg_count == 0 && update_output.membership_proof.is_none() {
            warn!("client update produced no messages, deferring packet batch until finality advances");
            return None;
        }

        self.relay
            .dst_chain()
            .finalize_batch(&mut update_output, &mut packet_msgs);

        let mut messages = update_output.messages;
        messages.extend(packet_msgs);

        info!(
            total = messages.len(),
            update = update_msg_count,
            packet = messages.len() - update_msg_count,
            "sending batch to tx_worker"
        );
        Some(messages)
    }

    #[instrument(skip_all, name = "build_timeout", fields(src_chain = %self.relay.src_chain().chain_label(), dst_chain = %self.relay.dst_chain().chain_label()))]
    #[expect(clippy::type_complexity, reason = "return type mirrors chain associated types")]
    async fn build_timeout_messages(
        &self,
        timed_out: Vec<PendingSend<SendEvent<R>>>,
        dst_height: &DstHeight<R>,
        message_proof_height: Option<&DstHeight<R>>,
    ) -> (
        Vec<<R::SrcChain as ChainTypes>::Message>,
        Vec<PendingSend<SendEvent<R>>>,
    ) {
        type SrcChain<R> = <R as Relay>::SrcChain;

        let relay = &self.relay;

        let results: Vec<_> = stream::iter(timed_out)
            .map(|ps| {
                let relay = relay.clone();
                let dst_height = dst_height.clone();
                let msg_height = message_proof_height.cloned();
                async move {
                    let pkt = <SrcChain<R> as PacketEvents>::packet_from_send_event(&ps.event);
                    let result = retry_proof_fetch(|| async {
                        relay
                            .build_timeout_packet_messages(pkt, &dst_height, msg_height.as_ref())
                            .await
                    })
                    .await;
                    (ps, result)
                }
            })
            .buffered(PROOF_FETCH_CONCURRENCY)
            .collect()
            .await;

        let mut messages = Vec::new();
        let mut failed = Vec::new();

        for (ps, result) in results {
            match result {
                Ok(msgs) if msgs.is_empty() => {}
                Ok(msgs) => {
                    messages.extend(msgs);
                }
                Err(e) => {
                    warn!(error = %e, "timeout proof fetch failed after retries, will retry");
                    failed.push(ps);
                }
            }
        }

        (messages, failed)
    }
}

type SendEvent<R> = <<R as Relay>::SrcChain as PacketEvents>::SendPacketEvent;
type WriteAckEvent<R> = <<R as Relay>::SrcChain as PacketEvents>::WriteAckEvent;
type WriteAckEvents<R> = Vec<WriteAckEvent<R>>;
type SrcHeight<R> = <<R as Relay>::SrcChain as ChainTypes>::Height;
type DstHeight<R> = <<R as Relay>::DstChain as ChainTypes>::Height;
type DstMessage<R> = <<R as Relay>::DstChain as ChainTypes>::Message;

#[instrument(skip_all, name = "retry_proof_fetch")]
async fn retry_proof_fetch<F, Fut, T>(f: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut attempt = 1;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if attempt < PROOF_FETCH_MAX_RETRIES => {
                debug!(attempt, error = %e, "proof fetch failed, retrying");
                tokio::time::sleep(PROOF_FETCH_RETRY_DELAY).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

#[async_trait]
impl<R> Worker for PacketWorker<R>
where
    R: Relay + RelayPacketBuilder,
    R::SrcChain: ClientQuery<DstCore<R>>
        + ClientMessageBuilder<
            DstCore<R>,
            CreateClientPayload = <R::DstChain as ClientPayloadBuilder<SrcCore<R>>>::CreateClientPayload,
            UpdateClientPayload = <R::DstChain as ClientPayloadBuilder<SrcCore<R>>>::UpdateClientPayload,
        >,
{
    fn name(&self) -> &'static str {
        "packet_worker"
    }

    #[instrument(skip_all, name = "packet_worker", fields(src_chain = %self.relay.src_chain().chain_label(), dst_chain = %self.relay.dst_chain().chain_label()))]
    async fn run(mut self) -> Result<()> {
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        let mut pending: Vec<PendingSend<SendEvent<R>>> = Vec::new();
        let mut pending_acks: WriteAckEvents<R> = Vec::new();
        info!("packet worker started");

        loop {
            let has_pending = !pending.is_empty() || !pending_acks.is_empty();

            let events = tokio::select! {
                Some(events) = self.receiver.recv() => events,
                () = self.token.cancelled() => break,
                () = tokio::time::sleep(PENDING_RECHECK_INTERVAL) => {
                    if !has_pending {
                        continue;
                    }
                    if !pending.is_empty() {
                        debug!(count = pending.len(), "re-checking pending send packets");
                    }
                    if !pending_acks.is_empty() {
                        debug!(count = pending_acks.len(), "retrying pending ack packets");
                    }
                    vec![]
                }
            };

            let mut write_acks = std::mem::take(&mut pending_acks);
            for event in events {
                match event {
                    IbcEvent::SendPacket(e) => pending.push(PendingSend {
                        event: e,
                        recv_sent_at: None,
                    }),
                    IbcEvent::WriteAck(e) => write_acks.push(e),
                }
            }

            if pending.is_empty() && write_acks.is_empty() {
                continue;
            }

            let Ok(dst_status) = self
                .relay
                .dst_chain()
                .query_chain_status()
                .await
                .inspect_err(
                    |e| warn!(error = %e, "failed to query dst chain status, skipping iteration"),
                )
            else {
                pending_acks = write_acks;
                continue;
            };
            let dst_timestamp_secs = DstChain::<R>::chain_status_timestamp_secs(&dst_status);

            let all_pending = std::mem::take(&mut pending);
            let mut new_sends = Vec::new();
            let mut in_flight = Vec::new();
            let mut in_flight_expired = Vec::new();
            let now = Instant::now();

            for ps in all_pending {
                match ps.recv_sent_at {
                    None => new_sends.push(ps),
                    Some(t) if now.duration_since(t) >= RECV_GRACE_PERIOD => {
                        in_flight_expired.push(ps);
                    }
                    Some(_) => in_flight.push(ps),
                }
            }

            if !in_flight_expired.is_empty() {
                let dst_height = match self.relay.dst_chain().query_latest_height().await {
                    Ok(h) => h,
                    Err(e) => {
                        warn!(error = %e, "failed to query dst height for confirmation check");
                        pending.extend(in_flight_expired);
                        pending.extend(in_flight);
                        pending.extend(new_sends);
                        pending_acks = write_acks;
                        continue;
                    }
                };
                let still_unconfirmed =
                    self.confirm_in_flight(in_flight_expired, &dst_height).await;
                new_sends.extend(still_unconfirmed);
            }

            let mut deliverable = Vec::new();
            let mut timed_out = Vec::new();
            let mut drift_zone = Vec::new();

            let drift_secs = self.relay.dst_chain().max_clock_drift().as_secs();

            for ps in new_sends {
                let pkt = <SrcChain<R> as PacketEvents>::packet_from_send_event(&ps.event);
                let ts = <SrcChain<R> as IbcTypes>::packet_timeout_timestamp(pkt);
                let ts_secs: u64 = ts.into();
                if ts_secs > 0 && dst_timestamp_secs >= ts_secs {
                    let seq = <SrcChain<R> as IbcTypes>::packet_sequence(pkt);
                    debug!(%seq, "packet timed out, will relay timeout");
                    timed_out.push(ps);
                } else if ts_secs > 0 && dst_timestamp_secs.saturating_add(drift_secs) >= ts_secs {
                    let seq = <SrcChain<R> as IbcTypes>::packet_sequence(pkt);
                    debug!(%seq, "packet in drift zone (drift buffer {drift_secs}s), deferring");
                    drift_zone.push(ps);
                } else {
                    deliverable.push(ps);
                }
            }

            // Packets in the drift zone are too close to timeout to deliver safely,
            // but the destination chain hasn't reached the timeout yet. Keep them
            // pending so they get re-evaluated on the next loop iteration.
            pending.extend(drift_zone);

            self.record_backlog_metrics(&in_flight, &deliverable, &timed_out);

            if !deliverable.is_empty() || !write_acks.is_empty() {
                match self.build_dst_update_client_payload().await {
                    Ok((src_height, message_proof_height, maybe_payload)) => {
                        let (recv_msgs, recv_built, recv_unbuilt, recv_entries) = self
                            .build_recv_messages_tracked(
                                deliverable,
                                &src_height,
                                message_proof_height.as_ref(),
                            )
                            .await;
                        pending.extend(recv_unbuilt);

                        let (ack_msgs, ack_built, ack_failed, ack_entries) = self
                            .build_ack_messages_tracked(
                                write_acks,
                                &src_height,
                                message_proof_height.as_ref(),
                            )
                            .await;
                        pending_acks.extend(ack_failed);

                        self.metrics.record_recv(recv_msgs.len());
                        self.metrics.record_ack(ack_msgs.len());

                        let packet_msgs: Vec<_> = recv_msgs.into_iter().chain(ack_msgs).collect();
                        let all_entries: Vec<_> =
                            recv_entries.into_iter().chain(ack_entries).collect();

                        if let Some(messages) = self
                            .build_dst_batch(maybe_payload, packet_msgs, &all_entries)
                            .await
                        {
                            if !messages.is_empty()
                                && send_or_cancel(
                                    &self.sender,
                                    DstTxRequest {
                                        messages,
                                        created_at: std::time::Instant::now(),
                                    },
                                    &self.token,
                                    "tx_worker",
                                )
                                .await
                            {
                                return Ok(());
                            }
                            let sent_at = Some(Instant::now());
                            pending.extend(
                                recv_built
                                    .into_iter()
                                    .map(|ps| PendingSend { recv_sent_at: sent_at, ..ps }),
                            );
                        } else {
                            pending.extend(recv_built);
                            pending_acks.extend(ack_built);
                        }
                    }
                    Err(e) => {
                        pending.extend(deliverable);
                        pending_acks.extend(write_acks);
                        warn!(error = %e, "failed to build dst update client payload, skipping recv/ack batch");
                    }
                }
            }

            if !timed_out.is_empty() {
                debug!(count = timed_out.len(), "relaying timeout packets");

                match self.build_src_update_client_messages().await {
                    Ok((dst_height, message_proof_height, src_update_msgs)) => {
                        let (timeout_msgs, timeout_failed) = self
                            .build_timeout_messages(
                                timed_out,
                                &dst_height,
                                message_proof_height.as_ref(),
                            )
                            .await;
                        pending.extend(timeout_failed);

                        if !timeout_msgs.is_empty() {
                            self.metrics.record_timeout(timeout_msgs.len());
                            let mut messages = src_update_msgs;
                            messages.extend(timeout_msgs);

                            if send_or_cancel(
                                &self.src_sender,
                                SrcTxRequest {
                                    messages,
                                    created_at: std::time::Instant::now(),
                                },
                                &self.token,
                                "src_tx_worker",
                            )
                            .await
                            {
                                return Ok(());
                            }
                        }
                    }
                    Err(e) => {
                        pending.extend(timed_out);
                        warn!(error = %e, "failed to build src update client messages, skipping timeout batch");
                    }
                }
            }

            pending.extend(in_flight);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::testhelpers::*;

    /// Mock clients report a latest height of 100.
    const TRUSTED: u64 = 100;

    fn make_worker(src: MockState, dst: MockState) -> PacketWorker<MockRelay> {
        let relay = MockRelay {
            src: MockChain::new(Arc::new(Mutex::new(src))),
            dst: MockChain::new(Arc::new(Mutex::new(dst))),
            src_client_id: "src-client".into(),
            dst_client_id: "dst-client".into(),
        };
        let (_events_tx, receiver) = mpsc::channel(1);
        let (sender, _dst_rx) = mpsc::channel(1);
        let (src_sender, _src_rx) = mpsc::channel(1);
        PacketWorker {
            relay: Arc::new(relay),
            receiver,
            sender,
            src_sender,
            token: CancellationToken::new(),
            metrics: PacketMetrics::new(&mercury_core::ChainLabel::new("mock")),
        }
    }

    fn lagging(prove_at_trusted: bool) -> MockState {
        MockState {
            latest_height: 90,
            prove_at_trusted,
            ..MockState::default()
        }
    }

    #[tokio::test]
    async fn dst_no_update_proves_at_src_height_by_default() {
        let worker = make_worker(lagging(false), MockState::default());
        let (proof, msg, payload) = worker.build_dst_update_client_payload().await.unwrap();
        assert_eq!((proof, msg, payload.is_none()), (90, None, true));
    }

    #[tokio::test]
    async fn dst_no_update_uses_chain_proof_height() {
        let worker = make_worker(lagging(true), MockState::default());
        let (proof, _, payload) = worker.build_dst_update_client_payload().await.unwrap();
        assert_eq!((proof, payload.is_none()), (TRUSTED, true));
    }

    #[tokio::test]
    async fn src_no_update_uses_chain_proof_height() {
        let worker = make_worker(MockState::default(), lagging(true));
        let (proof, msg, update) = worker.build_src_update_client_messages().await.unwrap();
        assert_eq!((proof, msg, update.len()), (TRUSTED, None, 0));
    }

    #[tokio::test]
    async fn src_update_proves_at_dst_height() {
        let dst = MockState {
            latest_height: 120,
            ..MockState::default()
        };
        let worker = make_worker(MockState::default(), dst);
        let (proof, msg, _) = worker.build_src_update_client_messages().await.unwrap();
        assert_eq!((proof, msg), (120, None));
    }

    struct Harness {
        events: mpsc::Sender<Vec<IbcEvent<MockRelay>>>,
        dst_rx: mpsc::Receiver<DstTxRequest<MockRelay>>,
        src_rx: mpsc::Receiver<SrcTxRequest<MockRelay>>,
        token: CancellationToken,
        handle: tokio::task::JoinHandle<Result<()>>,
        src: SharedState,
        dst: SharedState,
    }

    impl Harness {
        fn start(src: MockState, dst: MockState) -> Self {
            let (relay, src, dst) = mock_relay(src, dst);
            let (events, receiver) = mpsc::channel(8);
            let (sender, dst_rx) = mpsc::channel(8);
            let (src_sender, src_rx) = mpsc::channel(8);
            let token = CancellationToken::new();
            let worker = PacketWorker {
                relay,
                receiver,
                sender,
                src_sender,
                token: token.clone(),
                metrics: PacketMetrics::new(&mock_label()),
            };
            let handle = tokio::spawn(worker.run());
            Self {
                events,
                dst_rx,
                src_rx,
                token,
                handle,
                src,
                dst,
            }
        }

        async fn send(&self, events: Vec<IbcEvent<MockRelay>>) {
            self.events.send(events).await.unwrap();
        }

        async fn stop(self) {
            self.token.cancel();
            self.handle.await.unwrap().unwrap();
        }
    }

    fn send_packet(packet: MockPacket) -> IbcEvent<MockRelay> {
        IbcEvent::SendPacket(MockSendPacketEvent(packet))
    }

    fn write_ack(seq: u64) -> IbcEvent<MockRelay> {
        IbcEvent::WriteAck(MockWriteAckEvent(MockPacket::new(seq)))
    }

    fn builds(msgs: usize) -> MockState {
        MockState {
            build_msgs: msgs,
            ..MockState::default()
        }
    }

    /// Src ahead of the dst client (trusted at 100), so an update is needed.
    fn src_ahead() -> MockState {
        MockState {
            latest_height: 150,
            ..MockState::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn recv_sent_without_update_when_client_current() {
        let mut h = Harness::start(MockState::default(), builds(1));
        h.send(vec![send_packet(MockPacket::new(1)), write_ack(2)])
            .await;

        let req = recv(&mut h.dst_rx).await;
        assert_eq!(req.messages.len(), 2, "one recv + one ack message");
        assert_eq!(h.src.lock().unwrap().update_payload_calls, 0);
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn recv_batched_behind_client_update() {
        let dst = MockState {
            update_messages: 1,
            ..builds(2)
        };
        let mut h = Harness::start(src_ahead(), dst);
        h.send(vec![send_packet(MockPacket::new(1))]).await;

        let req = recv(&mut h.dst_rx).await;
        assert_eq!(req.messages.len(), 3, "update + two packet messages");
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn update_only_sent_when_no_packet_messages_built() {
        let dst = MockState {
            update_messages: 1,
            ..builds(0)
        };
        let mut h = Harness::start(src_ahead(), dst);
        h.send(vec![send_packet(MockPacket::new(1))]).await;

        let req = recv(&mut h.dst_rx).await;
        assert_eq!(req.messages.len(), 1);
        h.stop().await;
    }

    /// Receives within one pending recheck, i.e. without waiting out the
    /// in-flight grace period.
    async fn recv_on_recheck<T>(rx: &mut mpsc::Receiver<T>) -> T {
        tokio::time::timeout(PENDING_RECHECK_INTERVAL + Duration::from_secs(1), recv(rx))
            .await
            .expect("not retried on the next recheck")
    }

    #[tokio::test(start_paused = true)]
    async fn packet_batch_deferred_when_update_yields_nothing() {
        let mut h = Harness::start(src_ahead(), builds(1));
        h.send(vec![send_packet(MockPacket::new(1)), write_ack(2)])
            .await;

        tokio::time::sleep(Duration::from_secs(6)).await;
        assert!(h.dst_rx.try_recv().is_err());

        h.dst.lock().unwrap().update_messages = 1;
        let req = recv_on_recheck(&mut h.dst_rx).await;
        assert_eq!(req.messages.len(), 3, "update + recv + ack after retry");
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn update_message_error_keeps_batch_pending() {
        let dst = MockState {
            update_message_error: true,
            update_messages: 1,
            ..builds(1)
        };
        let mut h = Harness::start(src_ahead(), dst);
        h.send(vec![send_packet(MockPacket::new(1)), write_ack(2)])
            .await;

        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(h.dst_rx.try_recv().is_err());

        h.dst.lock().unwrap().update_message_error = false;
        let req = recv_on_recheck(&mut h.dst_rx).await;
        assert_eq!(req.messages.len(), 3, "update + recv + ack after retry");
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn update_message_error_still_relays_timeouts() {
        let src = MockState {
            build_msgs: 1,
            ..src_ahead()
        };
        let dst = MockState {
            timestamp_secs: 60,
            update_message_error: true,
            ..builds(1)
        };
        let mut h = Harness::start(src, dst);
        h.send(vec![
            send_packet(MockPacket::with_timeout(1, 50)),
            send_packet(MockPacket::new(2)),
        ])
        .await;

        assert_eq!(recv_on_recheck(&mut h.src_rx).await.messages.len(), 1);
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn dst_status_failure_keeps_acks_pending() {
        let dst = MockState {
            status_error: true,
            ..builds(1)
        };
        let mut h = Harness::start(MockState::default(), dst);
        h.send(vec![write_ack(2)]).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(h.dst_rx.try_recv().is_err());

        h.dst.lock().unwrap().status_error = false;
        assert_eq!(recv_on_recheck(&mut h.dst_rx).await.messages.len(), 1);
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn update_payload_error_keeps_packets_pending() {
        let src = MockState {
            update_payload_error: true,
            ..src_ahead()
        };
        let dst = MockState {
            update_messages: 1,
            ..builds(1)
        };
        let mut h = Harness::start(src, dst);
        h.send(vec![send_packet(MockPacket::new(1)), write_ack(2)])
            .await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(h.dst_rx.try_recv().is_err());

        h.src.lock().unwrap().update_payload_error = false;
        let req = recv(&mut h.dst_rx).await;
        assert_eq!(req.messages.len(), 3, "update + recv + ack after retry");
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn ack_build_failure_retries_then_succeeds_on_recheck() {
        let dst = MockState {
            build_error: true,
            ..builds(1)
        };
        let mut h = Harness::start(MockState::default(), dst);
        h.send(vec![write_ack(2)]).await;

        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(h.dst.lock().unwrap().build_calls, PROOF_FETCH_MAX_RETRIES);
        assert!(h.dst_rx.try_recv().is_err());

        h.dst.lock().unwrap().build_error = false;
        assert_eq!(recv(&mut h.dst_rx).await.messages.len(), 1);
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_packet_relayed_to_src() {
        let src = MockState {
            build_msgs: 1,
            ..MockState::default()
        };
        let dst = MockState {
            timestamp_secs: 60,
            ..builds(1)
        };
        let mut h = Harness::start(src, dst);
        h.send(vec![
            send_packet(MockPacket::with_timeout(1, 50)),
            send_packet(MockPacket::with_timeout(2, 1000)),
        ])
        .await;

        assert_eq!(recv(&mut h.src_rx).await.messages.len(), 1);
        assert_eq!(recv(&mut h.dst_rx).await.messages.len(), 1);
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_includes_src_client_update_when_lagging() {
        let src = MockState {
            build_msgs: 1,
            update_messages: 1,
            ..MockState::default()
        };
        let dst = MockState {
            latest_height: 150,
            timestamp_secs: 60,
            ..MockState::default()
        };
        let mut h = Harness::start(src, dst);
        h.send(vec![send_packet(MockPacket::with_timeout(1, 50))])
            .await;

        assert_eq!(recv(&mut h.src_rx).await.messages.len(), 2);
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_build_failure_retries_on_recheck() {
        let src = MockState {
            build_msgs: 1,
            build_error: true,
            ..MockState::default()
        };
        let dst = MockState {
            timestamp_secs: 60,
            ..MockState::default()
        };
        let mut h = Harness::start(src, dst);
        h.send(vec![send_packet(MockPacket::with_timeout(1, 50))])
            .await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(h.src_rx.try_recv().is_err());

        h.src.lock().unwrap().build_error = false;
        assert_eq!(recv(&mut h.src_rx).await.messages.len(), 1);
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn src_update_failure_keeps_timeouts_pending() {
        let src = MockState {
            build_msgs: 1,
            ..MockState::default()
        };
        let dst = MockState {
            latest_height: 150,
            timestamp_secs: 60,
            update_payload_error: true,
            ..MockState::default()
        };
        let mut h = Harness::start(src, dst);
        h.send(vec![send_packet(MockPacket::with_timeout(1, 50))])
            .await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(h.src_rx.try_recv().is_err());

        h.dst.lock().unwrap().update_payload_error = false;
        assert_eq!(recv(&mut h.src_rx).await.messages.len(), 1);
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn drift_zone_packet_deferred_until_timeout() {
        let src = MockState {
            build_msgs: 1,
            ..MockState::default()
        };
        let dst = MockState {
            timestamp_secs: 45,
            max_clock_drift_secs: 10,
            ..builds(1)
        };
        let mut h = Harness::start(src, dst);
        h.send(vec![send_packet(MockPacket::with_timeout(1, 50))])
            .await;

        tokio::time::sleep(Duration::from_secs(6)).await;
        assert!(
            h.dst_rx.try_recv().is_err(),
            "not delivered inside drift zone"
        );
        assert!(h.src_rx.try_recv().is_err(), "not yet timed out");

        h.dst.lock().unwrap().timestamp_secs = 50;
        assert_eq!(recv(&mut h.src_rx).await.messages.len(), 1);
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn dst_status_failure_keeps_send_packets_pending() {
        let dst = MockState {
            status_error: true,
            ..builds(1)
        };
        let mut h = Harness::start(MockState::default(), dst);
        h.send(vec![send_packet(MockPacket::new(1))]).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(h.dst_rx.try_recv().is_err());

        h.dst.lock().unwrap().status_error = false;
        assert_eq!(recv(&mut h.dst_rx).await.messages.len(), 1);
        h.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn closed_dst_channel_cancels_relay() {
        let h = Harness::start(MockState::default(), builds(1));
        let Harness {
            events,
            dst_rx,
            token,
            handle,
            ..
        } = h;
        drop(dst_rx);
        events
            .send(vec![send_packet(MockPacket::new(1))])
            .await
            .unwrap();
        handle.await.unwrap().unwrap();
        assert!(token.is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn closed_src_channel_cancels_relay() {
        let src = MockState {
            build_msgs: 1,
            ..MockState::default()
        };
        let dst = MockState {
            timestamp_secs: 60,
            ..MockState::default()
        };
        let h = Harness::start(src, dst);
        let Harness {
            events,
            src_rx,
            token,
            handle,
            ..
        } = h;
        drop(src_rx);
        events
            .send(vec![send_packet(MockPacket::with_timeout(1, 50))])
            .await
            .unwrap();
        handle.await.unwrap().unwrap();
        assert!(token.is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn retry_proof_fetch_gives_up_after_max_attempts() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result: Result<()> = retry_proof_fetch(|| async {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(eyre::eyre!("nope"))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.into_inner(), PROOF_FETCH_MAX_RETRIES);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_proof_fetch_returns_first_success() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = retry_proof_fetch(|| async {
            let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Err(eyre::eyre!("transient"))
            } else {
                Ok(n)
            }
        })
        .await;
        assert_eq!(result.unwrap(), 1);
    }
}
