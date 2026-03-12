use std::borrow::Borrow;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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
use mercury_chain_traits::relay::packet::{CanBuildAckPacketMessages, CanBuildReceivePacketMessages};
use mercury_chain_traits::types::{HasChainStatusType, HasMessageTypes, HasPacketTypes};
use mercury_core::error::{Error, Result};
use mercury_core::worker::Worker;

use crate::workers::TxRequest;

const PROOF_FETCH_CONCURRENCY: usize = 8;

pub struct PacketWorker<R: Relay> {
    pub relay: Arc<R>,
    pub receiver: mpsc::Receiver<Vec<IbcEvent<R>>>,
    pub sender: mpsc::Sender<TxRequest<R>>,
    pub token: CancellationToken,
}

impl<R> PacketWorker<R>
where
    R: Relay + CanBuildReceivePacketMessages + CanBuildAckPacketMessages,
    R::SrcChain: CanQueryChainStatus
        + HasChainStatusType
        + CanBuildUpdateClientPayload<R::DstChain>,
    R::DstChain: CanQueryChainStatus
        + HasChainStatusType
        + CanQueryClientState<R::SrcChain>
        + HasClientLatestHeight<R::SrcChain>
        + CanBuildUpdateClientMessage<R::SrcChain>,
    <R::SrcChain as HasPacketTypes<R::DstChain>>::Acknowledgement:
        Borrow<<R::DstChain as HasPacketTypes<R::SrcChain>>::Acknowledgement>,
{
    async fn build_update_client_messages(
        &self,
    ) -> Result<(
        <R::SrcChain as mercury_chain_traits::types::HasChainTypes>::Height,
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

    async fn build_packet_messages(
        &self,
        send_packets: SendEvents<R>,
        write_acks: WriteAckEvents<R>,
        src_height: &<R::SrcChain as mercury_chain_traits::types::HasChainTypes>::Height,
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
                    relay.build_receive_packet_messages(pkt, &src_height).await
                }
            })
            .buffered(PROOF_FETCH_CONCURRENCY)
            .filter_map(|r| async {
                match r {
                    Ok(msgs) => Some(msgs),
                    Err(e) => {
                        warn!(error = %e, "recv proof fetch failed");
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
                    relay.build_ack_packet_messages(pkt, ack.borrow(), &src_height).await
                }
            })
            .buffered(PROOF_FETCH_CONCURRENCY)
            .filter_map(|r| async {
                match r {
                    Ok(msgs) => Some(msgs),
                    Err(e) => {
                        warn!(error = %e, "ack proof fetch failed");
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
}

type SendEvents<R> =
    Vec<<<R as Relay>::SrcChain as CanExtractPacketEvents<<R as Relay>::DstChain>>::SendPacketEvent>;
type WriteAckEvents<R> =
    Vec<<<R as Relay>::SrcChain as CanExtractPacketEvents<<R as Relay>::DstChain>>::WriteAckEvent>;

fn classify_events<R: Relay>(
    events: Vec<IbcEvent<R>>,
) -> (SendEvents<R>, WriteAckEvents<R>) {
    type SrcChain<R> = <R as Relay>::SrcChain;
    type DstChain<R> = <R as Relay>::DstChain;

    let mut send_packets = Vec::new();
    let mut write_acks = Vec::new();
    for event in events {
        match event {
            IbcEvent::SendPacket(e) => send_packets.push(e),
            IbcEvent::WriteAck(e) => write_acks.push(e),
        }
    }

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    send_packets.retain(|e| {
        let pkt =
            <SrcChain<R> as CanExtractPacketEvents<DstChain<R>>>::packet_from_send_event(e);
        let ts =
            <SrcChain<R> as HasPacketTypes<DstChain<R>>>::packet_timeout_timestamp(pkt);
        if ts > 0 && now_secs >= ts {
            let seq =
                <SrcChain<R> as HasPacketTypes<DstChain<R>>>::packet_sequence(pkt);
            debug!(seq, "skipping timed-out packet");
            false
        } else {
            true
        }
    });

    (send_packets, write_acks)
}

#[async_trait]
impl<R> Worker for PacketWorker<R>
where
    R: Relay + CanBuildReceivePacketMessages + CanBuildAckPacketMessages,
    R::SrcChain: CanQueryChainStatus
        + HasChainStatusType
        + CanBuildUpdateClientPayload<R::DstChain>,
    R::DstChain: CanQueryChainStatus
        + HasChainStatusType
        + CanQueryClientState<R::SrcChain>
        + HasClientLatestHeight<R::SrcChain>
        + CanBuildUpdateClientMessage<R::SrcChain>,
    <R::SrcChain as HasPacketTypes<R::DstChain>>::Acknowledgement:
        Borrow<<R::DstChain as HasPacketTypes<R::SrcChain>>::Acknowledgement>,
{
    fn name(&self) -> &'static str {
        "packet_worker"
    }

    async fn run(mut self) -> Result<()> {
        loop {
            let events = tokio::select! {
                Some(events) = self.receiver.recv() => events,
                () = self.token.cancelled() => break,
            };

            if events.is_empty() {
                continue;
            }

            let (send_packets, write_acks) = classify_events::<R>(events);
            if send_packets.is_empty() && write_acks.is_empty() {
                continue;
            }

            let (src_height, update_msgs) = self.build_update_client_messages().await?;
            let update_msg_count = update_msgs.len();

            let packet_msgs = self
                .build_packet_messages(send_packets, write_acks, &src_height)
                .await;

            if !packet_msgs.is_empty() {
                let mut messages = update_msgs;
                messages.extend(packet_msgs);
                debug_assert!(messages.len() > update_msg_count);

                self.sender
                    .send(TxRequest { messages })
                    .await
                    .map_err(|_| Error::report(eyre::eyre!("tx_worker channel closed")))?;
            }
        }

        Ok(())
    }
}
