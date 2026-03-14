use std::borrow::Borrow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, warn};

use mercury_chain_traits::prelude::*;
use mercury_chain_traits::relay::{IbcEvent, Relay, RelayPacketBuilder};
use mercury_core::error::Result;
use mercury_core::worker::Worker;

use crate::workers::{DstTxRequest, SrcTxRequest};

type UpdateClientResult<Height, Message> = Result<(Height, Vec<Message>)>;
type BuildMessagesResult<Message, R> = (Vec<Message>, Vec<PendingSend<SendEvent<R>>>);

const PROOF_FETCH_CONCURRENCY: usize = 8;
const PROOF_FETCH_MAX_RETRIES: usize = 3;
const PROOF_FETCH_RETRY_DELAY: Duration = Duration::from_millis(500);

const PENDING_RECHECK_INTERVAL: Duration = Duration::from_secs(5);
const RECV_GRACE_PERIOD: Duration = Duration::from_secs(15);

/// Receives IBC events and builds relay messages (recv, ack, timeout).
pub struct PacketWorker<R: Relay> {
    pub relay: Arc<R>,
    pub receiver: mpsc::Receiver<Vec<IbcEvent<R>>>,
    pub sender: mpsc::Sender<DstTxRequest<R>>,
    pub src_sender: mpsc::Sender<SrcTxRequest<R>>,
    pub token: CancellationToken,
}

struct PendingSend<E> {
    event: E,
    recv_sent_at: Option<Instant>,
}

impl<R> PacketWorker<R>
where
    R: Relay + RelayPacketBuilder,
    <R::SrcChain as IbcTypes<R::DstChain>>::Acknowledgement:
        Borrow<<R::DstChain as IbcTypes<R::SrcChain>>::Acknowledgement>,
{
    #[instrument(skip_all, name = "build_dst_update_client")]
    async fn build_dst_update_client_messages(
        &self,
    ) -> UpdateClientResult<<R::SrcChain as ChainTypes>::Height, <R::DstChain as ChainTypes>::Message>
    {
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
            return Ok((src_height, vec![]));
        }

        let update_payload = self
            .relay
            .src_chain()
            .build_update_client_payload(&trusted_height, &src_height, &client_state)
            .await?;
        let update_msgs = self
            .relay
            .dst_chain()
            .build_update_client_message(self.relay.dst_client_id(), update_payload)
            .await?;

        Ok((src_height, update_msgs))
    }

    #[instrument(skip_all, name = "build_src_update_client")]
    async fn build_src_update_client_messages(
        &self,
    ) -> UpdateClientResult<<R::DstChain as ChainTypes>::Height, <R::SrcChain as ChainTypes>::Message>
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
            return Ok((dst_height, vec![]));
        }

        let update_payload = self
            .relay
            .dst_chain()
            .build_update_client_payload(&trusted_height, &dst_height, &client_state)
            .await?;
        let update_msgs = self
            .relay
            .src_chain()
            .build_update_client_message(self.relay.src_client_id(), update_payload)
            .await?;

        Ok((dst_height, update_msgs))
    }

    #[instrument(skip_all, name = "build_recv_tracked")]
    async fn build_recv_messages_tracked(
        &self,
        send_packets: Vec<PendingSend<SendEvent<R>>>,
        src_height: &<R::SrcChain as ChainTypes>::Height,
    ) -> BuildMessagesResult<<R::DstChain as ChainTypes>::Message, R> {
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        let relay = &self.relay;

        let results: Vec<_> = stream::iter(send_packets)
            .map(|ps| {
                let relay = relay.clone();
                let src_height = src_height.clone();
                async move {
                    let pkt = <SrcChain<R> as PacketEvents<DstChain<R>>>::packet_from_send_event(
                        &ps.event,
                    );
                    let result = retry_proof_fetch(|| async {
                        relay.build_receive_packet_messages(pkt, &src_height).await
                    })
                    .await;
                    (ps, result)
                }
            })
            .buffered(PROOF_FETCH_CONCURRENCY)
            .collect()
            .await;

        let mut messages = Vec::new();
        let mut still_pending = Vec::new();
        let now = Instant::now();

        for (mut ps, result) in results {
            match result {
                Ok(msgs) if msgs.is_empty() => {}
                Ok(msgs) => {
                    messages.extend(msgs);
                    ps.recv_sent_at = Some(now);
                    still_pending.push(ps);
                }
                Err(e) => {
                    warn!(error = %e, "recv proof fetch failed after retries, will retry");
                    still_pending.push(ps);
                }
            }
        }

        (messages, still_pending)
    }

    /// Queries receipt on dst chain for in-flight packets past grace period.
    /// Receipt exists → drop. No receipt → reset for re-classification.
    #[instrument(skip_all, name = "confirm_in_flight")]
    async fn confirm_in_flight(
        &self,
        in_flight: Vec<PendingSend<SendEvent<R>>>,
        dst_height: &<R::DstChain as ChainTypes>::Height,
    ) -> Vec<PendingSend<SendEvent<R>>> {
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        let relay = &self.relay;

        let results: Vec<_> = stream::iter(in_flight)
            .map(|ps| {
                let relay = relay.clone();
                let dst_height = dst_height.clone();
                async move {
                    let pkt = <SrcChain<R> as PacketEvents<DstChain<R>>>::packet_from_send_event(
                        &ps.event,
                    );
                    let seq = <SrcChain<R> as IbcTypes<DstChain<R>>>::packet_sequence(pkt);
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
                    debug!(seq, "receipt exists on dst, packet delivered");
                }
                Ok(_) => {
                    still_pending.push(PendingSend {
                        event: ps.event,
                        recv_sent_at: None,
                    });
                }
                Err(e) => {
                    debug!(seq, error = %e, "receipt query failed, keeping pending");
                    still_pending.push(ps);
                }
            }
        }

        still_pending
    }

    #[instrument(skip_all, name = "build_ack_tracked")]
    async fn build_ack_messages_tracked(
        &self,
        write_acks: WriteAckEvents<R>,
        src_height: &<R::SrcChain as ChainTypes>::Height,
    ) -> (Vec<<R::DstChain as ChainTypes>::Message>, WriteAckEvents<R>) {
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        let relay = &self.relay;

        let results: Vec<_> = stream::iter(write_acks)
            .map(|e| {
                let relay = relay.clone();
                let src_height = src_height.clone();
                async move {
                    let (pkt, ack) =
                        <SrcChain<R> as PacketEvents<DstChain<R>>>::packet_from_write_ack_event(&e);
                    let result = retry_proof_fetch(|| async {
                        relay
                            .build_ack_packet_messages(pkt, ack.borrow(), &src_height)
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
        let mut failed = Vec::new();

        for (event, result) in results {
            match result {
                Ok(msgs) if msgs.is_empty() => {}
                Ok(msgs) => {
                    messages.extend(msgs);
                }
                Err(e) => {
                    warn!(error = %e, "ack proof fetch failed after retries, will retry");
                    failed.push(event);
                }
            }
        }

        (messages, failed)
    }

    #[instrument(skip_all, name = "build_timeout")]
    async fn build_timeout_messages(
        &self,
        timed_out: Vec<PendingSend<SendEvent<R>>>,
        dst_height: &<R::DstChain as ChainTypes>::Height,
    ) -> BuildMessagesResult<<R::SrcChain as ChainTypes>::Message, R> {
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        let relay = &self.relay;

        let results: Vec<_> = stream::iter(timed_out)
            .map(|ps| {
                let relay = relay.clone();
                let dst_height = dst_height.clone();
                async move {
                    let pkt = <SrcChain<R> as PacketEvents<DstChain<R>>>::packet_from_send_event(
                        &ps.event,
                    );
                    let result = retry_proof_fetch(|| async {
                        relay.build_timeout_packet_messages(pkt, &dst_height).await
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

type SendEvent<R> =
    <<R as Relay>::SrcChain as PacketEvents<<R as Relay>::DstChain>>::SendPacketEvent;
type WriteAckEvents<R> =
    Vec<<<R as Relay>::SrcChain as PacketEvents<<R as Relay>::DstChain>>::WriteAckEvent>;

#[instrument(skip_all, name = "retry_proof_fetch")]
async fn retry_proof_fetch<F, Fut, T>(f: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_err = None;
    for attempt in 0..PROOF_FETCH_MAX_RETRIES {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt + 1 < PROOF_FETCH_MAX_RETRIES {
                    debug!(attempt = attempt + 1, error = %e, "proof fetch failed, retrying");
                    tokio::time::sleep(PROOF_FETCH_RETRY_DELAY).await;
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| eyre::eyre!("retry loop completed without attempting")))
}

#[async_trait]
impl<R> Worker for PacketWorker<R>
where
    R: Relay + RelayPacketBuilder,
    <R::SrcChain as IbcTypes<R::DstChain>>::Acknowledgement:
        Borrow<<R::DstChain as IbcTypes<R::SrcChain>>::Acknowledgement>,
{
    fn name(&self) -> &'static str {
        "packet_worker"
    }

    #[instrument(skip_all, name = "packet_worker")]
    async fn run(mut self) -> Result<()> {
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        let mut pending: Vec<PendingSend<SendEvent<R>>> = Vec::new();
        let mut pending_acks: WriteAckEvents<R> = Vec::new();

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

            let dst_status = match self.relay.dst_chain().query_chain_status().await {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, "failed to query dst chain status, skipping iteration");
                    continue;
                }
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
                        continue;
                    }
                };
                let still_unconfirmed =
                    self.confirm_in_flight(in_flight_expired, &dst_height).await;
                new_sends.extend(still_unconfirmed);
            }

            let mut deliverable = Vec::new();
            let mut timed_out = Vec::new();

            for ps in new_sends {
                let pkt =
                    <SrcChain<R> as PacketEvents<DstChain<R>>>::packet_from_send_event(&ps.event);
                let ts = <SrcChain<R> as IbcTypes<DstChain<R>>>::packet_timeout_timestamp(pkt);
                if ts > 0 && dst_timestamp_secs >= ts {
                    let seq = <SrcChain<R> as IbcTypes<DstChain<R>>>::packet_sequence(pkt);
                    debug!(seq, "packet timed out, will relay timeout");
                    timed_out.push(ps);
                } else {
                    deliverable.push(ps);
                }
            }

            if !deliverable.is_empty() || !write_acks.is_empty() {
                match self.build_dst_update_client_messages().await {
                    Ok((src_height, update_msgs)) => {
                        let update_msg_count = update_msgs.len();

                        let (recv_msgs, recv_pending) = self
                            .build_recv_messages_tracked(deliverable, &src_height)
                            .await;
                        pending.extend(recv_pending);

                        let (ack_msgs, ack_failed) = self
                            .build_ack_messages_tracked(write_acks, &src_height)
                            .await;
                        pending_acks.extend(ack_failed);

                        let mut messages = update_msgs;
                        messages.extend(recv_msgs);
                        messages.extend(ack_msgs);

                        if messages.len() > update_msg_count {
                            self.sender
                                .send(DstTxRequest { messages })
                                .await
                                .map_err(|_| eyre::eyre!("tx_worker channel closed"))?;
                        }
                    }
                    Err(e) => {
                        pending.extend(deliverable);
                        pending_acks.extend(write_acks);
                        warn!(error = %e, "failed to build dst update client messages, skipping recv/ack batch");
                    }
                }
            }

            if !timed_out.is_empty() {
                debug!(count = timed_out.len(), "relaying timeout packets");

                match self.build_src_update_client_messages().await {
                    Ok((dst_height, src_update_msgs)) => {
                        let (timeout_msgs, timeout_failed) =
                            self.build_timeout_messages(timed_out, &dst_height).await;
                        pending.extend(timeout_failed);

                        if !timeout_msgs.is_empty() {
                            let mut messages = src_update_msgs;
                            messages.extend(timeout_msgs);

                            self.src_sender
                                .send(SrcTxRequest { messages })
                                .await
                                .map_err(|_| eyre::eyre!("src_tx_worker channel closed"))?;
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
