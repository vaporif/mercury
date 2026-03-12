use std::borrow::Borrow;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use mercury_chain_traits::events::CanExtractPacketEvents;
use mercury_chain_traits::message_builders::CanBuildUpdateClientMessage;
use mercury_chain_traits::payload_builders::CanBuildUpdateClientPayload;
use mercury_chain_traits::queries::{
    CanQueryChainStatus, CanQueryClientState, HasClientLatestHeight,
};
use mercury_chain_traits::relay::context::Relay;
use mercury_chain_traits::relay::ibc_event::IbcEvent;
use mercury_chain_traits::relay::packet::{
    CanBuildAckPacketMessages, CanBuildReceivePacketMessages, CanBuildTimeoutPacketMessages,
};
use mercury_chain_traits::types::HasChainStatusType;
use mercury_chain_traits::types::{HasChainTypes, HasMessageTypes, HasPacketTypes};
use mercury_core::error::{Error, Result};
use mercury_core::worker::Worker;

use crate::workers::{DstTxRequest, SrcTxRequest};

const PROOF_FETCH_CONCURRENCY: usize = 8;
const PROOF_FETCH_MAX_RETRIES: usize = 3;
const PROOF_FETCH_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Receives IBC events and builds relay messages (recv, ack, timeout).
pub struct PacketWorker<R: Relay> {
    pub relay: Arc<R>,
    pub receiver: mpsc::Receiver<Vec<IbcEvent<R>>>,
    pub sender: mpsc::Sender<DstTxRequest<R>>,
    pub src_sender: mpsc::Sender<SrcTxRequest<R>>,
    pub token: CancellationToken,
}

impl<R> PacketWorker<R>
where
    R: Relay
        + CanBuildReceivePacketMessages
        + CanBuildAckPacketMessages
        + CanBuildTimeoutPacketMessages,
    R::SrcChain: CanBuildUpdateClientPayload<R::DstChain>
        + CanQueryClientState<R::DstChain>
        + HasClientLatestHeight<R::DstChain>
        + CanBuildUpdateClientMessage<R::DstChain>,
    R::DstChain: CanQueryChainStatus
        + CanQueryClientState<R::SrcChain>
        + HasClientLatestHeight<R::SrcChain>
        + CanBuildUpdateClientMessage<R::SrcChain>
        + CanBuildUpdateClientPayload<R::SrcChain>,
    <R::SrcChain as HasPacketTypes<R::DstChain>>::Acknowledgement:
        Borrow<<R::DstChain as HasPacketTypes<R::SrcChain>>::Acknowledgement>,
{
    async fn build_dst_update_client_messages(
        &self,
    ) -> Result<(
        <R::SrcChain as HasChainTypes>::Height,
        Vec<<R::DstChain as HasMessageTypes>::Message>,
    )> {
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
            .build_update_client_payload(&trusted_height, &src_height)
            .await?;
        let update_msgs = self
            .relay
            .dst_chain()
            .build_update_client_message(self.relay.dst_client_id(), update_payload)
            .await?;

        Ok((src_height, update_msgs))
    }

    async fn build_src_update_client_messages(
        &self,
    ) -> Result<(
        <R::DstChain as HasChainTypes>::Height,
        Vec<<R::SrcChain as HasMessageTypes>::Message>,
    )> {
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
            .build_update_client_payload(&trusted_height, &dst_height)
            .await?;
        let update_msgs = self
            .relay
            .src_chain()
            .build_update_client_message(self.relay.src_client_id(), update_payload)
            .await?;

        Ok((dst_height, update_msgs))
    }

    async fn build_recv_and_ack_messages(
        &self,
        send_packets: SendEvents<R>,
        write_acks: WriteAckEvents<R>,
        src_height: &<R::SrcChain as HasChainTypes>::Height,
    ) -> Vec<<R::DstChain as HasMessageTypes>::Message> {
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        let relay = &self.relay;

        let recv_msgs: Vec<Vec<_>> = stream::iter(send_packets)
            .map(|e| {
                let relay = relay.clone();
                let src_height = src_height.clone();
                async move {
                    let pkt = <SrcChain<R> as CanExtractPacketEvents<DstChain<R>>>::packet_from_send_event(&e);
                    retry_proof_fetch(|| async {
                        relay.build_receive_packet_messages(pkt, &src_height).await
                    })
                    .await
                }
            })
            .buffered(PROOF_FETCH_CONCURRENCY)
            .filter_map(|r| async {
                match r {
                    Ok(msgs) => Some(msgs),
                    Err(e) => {
                        warn!(error = %e, "recv proof fetch failed after retries");
                        None
                    }
                }
            })
            .collect()
            .await;

        let ack_msgs: Vec<Vec<_>> = stream::iter(write_acks)
            .map(|e| {
                let relay = relay.clone();
                let src_height = src_height.clone();
                async move {
                    let (pkt, ack) = <SrcChain<R> as CanExtractPacketEvents<DstChain<R>>>::packet_from_write_ack_event(&e);
                    retry_proof_fetch(|| async {
                        relay.build_ack_packet_messages(pkt, ack.borrow(), &src_height).await
                    })
                    .await
                }
            })
            .buffered(PROOF_FETCH_CONCURRENCY)
            .filter_map(|r| async {
                match r {
                    Ok(msgs) => Some(msgs),
                    Err(e) => {
                        warn!(error = %e, "ack proof fetch failed after retries");
                        None
                    }
                }
            })
            .collect()
            .await;

        let mut messages = Vec::new();
        for batch in recv_msgs {
            messages.extend(batch);
        }
        for batch in ack_msgs {
            messages.extend(batch);
        }
        messages
    }

    async fn build_timeout_messages(
        &self,
        timed_out: SendEvents<R>,
        dst_height: &<R::DstChain as HasChainTypes>::Height,
    ) -> Vec<<R::SrcChain as HasMessageTypes>::Message> {
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        let relay = &self.relay;

        let timeout_msgs: Vec<Vec<_>> = stream::iter(timed_out)
            .map(|e| {
                let relay = relay.clone();
                let dst_height = dst_height.clone();
                async move {
                    let pkt = <SrcChain<R> as CanExtractPacketEvents<DstChain<R>>>::packet_from_send_event(&e);
                    retry_proof_fetch(|| async {
                        relay.build_timeout_packet_messages(pkt, &dst_height).await
                    })
                    .await
                }
            })
            .buffered(PROOF_FETCH_CONCURRENCY)
            .filter_map(|r| async {
                match r {
                    Ok(msgs) => Some(msgs),
                    Err(e) => {
                        warn!(error = %e, "timeout proof fetch failed after retries");
                        None
                    }
                }
            })
            .collect()
            .await;

        let mut messages = Vec::new();
        for batch in timeout_msgs {
            messages.extend(batch);
        }
        messages
    }
}

type SendEvents<R> = Vec<
    <<R as Relay>::SrcChain as CanExtractPacketEvents<<R as Relay>::DstChain>>::SendPacketEvent,
>;
type WriteAckEvents<R> =
    Vec<<<R as Relay>::SrcChain as CanExtractPacketEvents<<R as Relay>::DstChain>>::WriteAckEvent>;

/// Classify events into send packets, timed-out send packets, and write acks.
/// Uses the destination chain's timestamp for timeout detection instead of local time.
fn classify_events<R: Relay>(
    events: Vec<IbcEvent<R>>,
    dst_timestamp_secs: u64,
) -> (SendEvents<R>, SendEvents<R>, WriteAckEvents<R>) {
    type SrcChain<R> = <R as Relay>::SrcChain;
    type DstChain<R> = <R as Relay>::DstChain;

    let mut send_packets = Vec::new();
    let mut timed_out = Vec::new();
    let mut write_acks = Vec::new();
    for event in events {
        match event {
            IbcEvent::SendPacket(e) => {
                let pkt =
                    <SrcChain<R> as CanExtractPacketEvents<DstChain<R>>>::packet_from_send_event(
                        &e,
                    );
                let ts =
                    <SrcChain<R> as HasPacketTypes<DstChain<R>>>::packet_timeout_timestamp(pkt);
                if ts > 0 && dst_timestamp_secs >= ts {
                    let seq = <SrcChain<R> as HasPacketTypes<DstChain<R>>>::packet_sequence(pkt);
                    debug!(seq, "packet timed out, will relay timeout");
                    timed_out.push(e);
                } else {
                    send_packets.push(e);
                }
            }
            IbcEvent::WriteAck(e) => write_acks.push(e),
        }
    }

    (send_packets, timed_out, write_acks)
}

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
    Err(last_err.unwrap())
}

#[async_trait]
impl<R> Worker for PacketWorker<R>
where
    R: Relay
        + CanBuildReceivePacketMessages
        + CanBuildAckPacketMessages
        + CanBuildTimeoutPacketMessages,
    R::SrcChain: CanBuildUpdateClientPayload<R::DstChain>
        + CanQueryClientState<R::DstChain>
        + HasClientLatestHeight<R::DstChain>
        + CanBuildUpdateClientMessage<R::DstChain>,
    R::DstChain: CanQueryChainStatus
        + CanQueryClientState<R::SrcChain>
        + HasClientLatestHeight<R::SrcChain>
        + CanBuildUpdateClientMessage<R::SrcChain>
        + CanBuildUpdateClientPayload<R::SrcChain>,
    <R::SrcChain as HasPacketTypes<R::DstChain>>::Acknowledgement:
        Borrow<<R::DstChain as HasPacketTypes<R::SrcChain>>::Acknowledgement>,
{
    fn name(&self) -> &'static str {
        "packet_worker"
    }

    async fn run(mut self) -> Result<()> {
        type DstChain<R> = <R as Relay>::DstChain;

        loop {
            let events = tokio::select! {
                Some(events) = self.receiver.recv() => events,
                () = self.token.cancelled() => break,
            };

            if events.is_empty() {
                continue;
            }

            // Use destination chain's timestamp for timeout detection
            let dst_status = match self.relay.dst_chain().query_chain_status().await {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, "failed to query dst chain status, skipping batch");
                    continue;
                }
            };
            let dst_timestamp_secs = DstChain::<R>::chain_status_timestamp_secs(&dst_status);

            let (send_packets, timed_out, write_acks) =
                classify_events::<R>(events, dst_timestamp_secs);

            // Handle receive + ack packets (dst-bound)
            if !send_packets.is_empty() || !write_acks.is_empty() {
                match self.build_dst_update_client_messages().await {
                    Ok((src_height, update_msgs)) => {
                        let update_msg_count = update_msgs.len();

                        let packet_msgs = self
                            .build_recv_and_ack_messages(send_packets, write_acks, &src_height)
                            .await;

                        if !packet_msgs.is_empty() {
                            let mut messages = update_msgs;
                            messages.extend(packet_msgs);
                            debug_assert!(messages.len() > update_msg_count);

                            self.sender
                                .send(DstTxRequest { messages })
                                .await
                                .map_err(|_| {
                                    Error::report(eyre::eyre!("tx_worker channel closed"))
                                })?;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to build dst update client messages, skipping recv/ack batch");
                    }
                }
            }

            // Handle timed-out packets (src-bound)
            if !timed_out.is_empty() {
                debug!(count = timed_out.len(), "relaying timeout packets");

                match self.build_src_update_client_messages().await {
                    Ok((dst_height, src_update_msgs)) => {
                        let timeout_msgs =
                            self.build_timeout_messages(timed_out, &dst_height).await;

                        if !timeout_msgs.is_empty() {
                            let mut messages = src_update_msgs;
                            messages.extend(timeout_msgs);

                            self.src_sender
                                .send(SrcTxRequest { messages })
                                .await
                                .map_err(|_| {
                                    Error::report(eyre::eyre!("src_tx_worker channel closed"))
                                })?;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to build src update client messages, skipping timeout batch");
                    }
                }
            }
        }

        Ok(())
    }
}
