use alloy::primitives::{B256, U256, keccak256};
use alloy::providers::{Provider, WsConnect};
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use async_trait::async_trait;
use eyre::Context;
use futures::StreamExt;
use mercury_chain_traits::events::{BlockEvents, PacketEvents};
use mercury_core::error::Result;

use crate::chain::EthereumChain;
use crate::contracts::{ICS26Router, IICS26RouterMsgs};
use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};

use crate::types::{
    BlockNumber, EvmAcknowledgement, EvmClientId, EvmEvent, EvmHeight, EvmPacket, EvmPayload,
    EvmSendPacketEvent, EvmWriteAckEvent,
};

fn sol_packet_to_evm(p: &IICS26RouterMsgs::Packet) -> EvmPacket {
    EvmPacket {
        source_client: p.sourceClient.clone(),
        dest_client: p.destClient.clone(),
        sequence: PacketSequence(p.sequence),
        timeout_timestamp: TimeoutTimestamp(p.timeoutTimestamp),
        payloads: p
            .payloads
            .iter()
            .map(|pl| EvmPayload {
                source_port: Port(pl.sourcePort.clone()),
                dest_port: Port(pl.destPort.clone()),
                version: pl.version.clone(),
                encoding: pl.encoding.clone(),
                value: pl.value.to_vec(),
            })
            .collect(),
    }
}

#[async_trait]
impl PacketEvents for EthereumChain {
    type SendPacketEvent = EvmSendPacketEvent;
    type WriteAckEvent = EvmWriteAckEvent;

    fn try_extract_send_packet_event(event: &EvmEvent) -> Option<EvmSendPacketEvent> {
        if event.topics.first() != Some(&ICS26Router::SendPacket::SIGNATURE_HASH) {
            return None;
        }
        let log = alloy::primitives::Log::new_unchecked(
            event.address,
            event.topics.clone(),
            event.data.clone().into(),
        );
        let decoded = ICS26Router::SendPacket::decode_log(&log)
            .inspect_err(|e| tracing::warn!(error = %e, "failed to decode SendPacket event"))
            .ok()?;
        Some(EvmSendPacketEvent {
            packet: sol_packet_to_evm(&decoded.data.packet),
            block_number: event.block_number,
        })
    }

    fn try_extract_write_ack_event(event: &EvmEvent) -> Option<EvmWriteAckEvent> {
        if event.topics.first() != Some(&ICS26Router::WriteAcknowledgement::SIGNATURE_HASH) {
            return None;
        }
        let log = alloy::primitives::Log::new_unchecked(
            event.address,
            event.topics.clone(),
            event.data.clone().into(),
        );
        let decoded = ICS26Router::WriteAcknowledgement::decode_log(&log)
            .inspect_err(
                |e| tracing::warn!(error = %e, "failed to decode WriteAcknowledgement event"),
            )
            .ok()?;
        // The Eureka contract currently only supports single-payload packets
        // (enforced by IBCMultiPayloadPacketNotSupported), so exactly one ack is expected.
        let ack_bytes = decoded
            .data
            .acknowledgements
            .into_iter()
            .next()
            .map(|a| a.to_vec())?;
        Some(EvmWriteAckEvent {
            packet: sol_packet_to_evm(&decoded.data.packet),
            ack: EvmAcknowledgement(ack_bytes),
            block_number: event.block_number,
        })
    }

    fn packet_from_send_event(event: &EvmSendPacketEvent) -> &EvmPacket {
        &event.packet
    }

    fn packet_from_write_ack_event(event: &EvmWriteAckEvent) -> (&EvmPacket, &EvmAcknowledgement) {
        (&event.packet, &event.ack)
    }

    async fn query_block_events(&self, height: &EvmHeight) -> Result<Vec<EvmEvent>> {
        let filter = Filter::new()
            .address(self.router_address)
            .from_block(height.0)
            .to_block(height.0);

        let logs = self
            .rpc_guard
            .guarded(|| async {
                self.provider
                    .get_logs(&filter)
                    .await
                    .wrap_err("querying block logs")
            })
            .await?;

        Ok(logs.iter().map(EvmEvent::from_alloy_log).collect())
    }

    async fn query_send_packet_event(
        &self,
        client_id: &EvmClientId,
        sequence: PacketSequence,
    ) -> Result<Option<EvmSendPacketEvent>> {
        let filter = Filter::new()
            .address(self.router_address)
            .event_signature(ICS26Router::SendPacket::SIGNATURE_HASH)
            .topic1(keccak256(client_id.0.as_bytes()))
            .topic2(B256::from(U256::from(sequence.0)))
            .from_block(self.config.deployment_block);

        let logs = self
            .rpc_guard
            .guarded(|| async {
                self.provider
                    .get_logs(&filter)
                    .await
                    .wrap_err("querying SendPacket event")
            })
            .await?;

        let event = logs.iter().find_map(|log| {
            let decoded = ICS26Router::SendPacket::decode_log(log.as_ref()).ok()?;
            Some(EvmSendPacketEvent {
                packet: sol_packet_to_evm(&decoded.data.packet),
                block_number: BlockNumber(log.block_number?),
            })
        });

        if logs.is_empty() {
            return Ok(None);
        }

        if event.is_none() {
            tracing::warn!(
                client_id = %client_id,
                sequence = %sequence,
                "matched SendPacket log but failed to decode"
            );
        }

        Ok(event)
    }

    async fn subscribe_block_events(
        &self,
    ) -> Result<Option<mercury_chain_traits::events::BlockEventStream<EvmHeight, EvmEvent>>> {
        let Some(ws_addr) = &self.config.ws_addr else {
            return Ok(None);
        };

        let ws = WsConnect::new(ws_addr.clone());
        let ws_provider = alloy::providers::ProviderBuilder::new()
            .connect_ws(ws)
            .await
            .map_err(|e| eyre::eyre!("ethereum websocket connect failed: {e}"))?;

        let filter = Filter::new().address(self.router_address);
        let sub = ws_provider
            .subscribe_logs(&filter)
            .await
            .map_err(|e| eyre::eyre!("ethereum log subscription failed: {e}"))?;

        let log_stream = sub.into_stream();
        let flush_timeout = self.config.block_time() * 2;

        let stream = futures::stream::unfold(
            (log_stream, None::<(u64, Vec<EvmEvent>)>, flush_timeout),
            |(mut logs, mut pending, flush_timeout)| async move {
                let flush = |pending: (u64, Vec<EvmEvent>), state| {
                    let flushed = BlockEvents {
                        height: EvmHeight(pending.0),
                        events: pending.1,
                    };
                    Some((Ok(flushed), state))
                };

                loop {
                    let next = tokio::time::timeout(flush_timeout, logs.next()).await;

                    match next {
                        Ok(Some(log)) => {
                            if log.removed {
                                continue;
                            }

                            let Some(block_number) = log.block_number else {
                                continue;
                            };

                            let event = EvmEvent::from_alloy_log(&log);

                            match &mut pending {
                                Some((pending_block, pending_events))
                                    if *pending_block == block_number =>
                                {
                                    pending_events.push(event);
                                }
                                Some((pending_block, pending_events)) => {
                                    let flushed = BlockEvents {
                                        height: EvmHeight(*pending_block),
                                        events: std::mem::take(pending_events),
                                    };
                                    *pending_block = block_number;
                                    *pending_events = vec![event];
                                    return Some((Ok(flushed), (logs, pending, flush_timeout)));
                                }
                                None => {
                                    pending = Some((block_number, vec![event]));
                                }
                            }
                        }
                        Ok(None) => {
                            return pending
                                .take()
                                .and_then(|p| flush(p, (logs, None, flush_timeout)));
                        }
                        Err(_timeout) => {
                            if let Some(p) = pending.take() {
                                return flush(p, (logs, None, flush_timeout));
                            }
                        }
                    }
                }
            },
        );

        Ok(Some(Box::pin(stream)))
    }
}
