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

        logs.iter()
            .map(EvmEvent::from_alloy_log)
            .collect::<eyre::Result<Vec<_>>>()
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

    async fn query_write_ack_event(
        &self,
        client_id: &EvmClientId,
        sequence: PacketSequence,
    ) -> Result<Option<EvmWriteAckEvent>> {
        let filter = Filter::new()
            .address(self.router_address)
            .event_signature(ICS26Router::WriteAcknowledgement::SIGNATURE_HASH)
            .topic2(B256::from(U256::from(sequence.0)))
            .from_block(self.config.deployment_block);

        let logs = self
            .rpc_guard
            .guarded(|| async {
                self.provider
                    .get_logs(&filter)
                    .await
                    .wrap_err("querying WriteAcknowledgement event")
            })
            .await?;

        if logs.is_empty() {
            return Ok(None);
        }

        let event = logs.iter().find_map(|log| {
            let decoded = ICS26Router::WriteAcknowledgement::decode_log(log.as_ref()).ok()?;
            if decoded.data.packet.destClient != client_id.0 {
                return None;
            }
            let ack_bytes = decoded.data.acknowledgements.into_iter().next()?.to_vec();
            Some(EvmWriteAckEvent {
                packet: sol_packet_to_evm(&decoded.data.packet),
                ack: EvmAcknowledgement(ack_bytes),
                block_number: BlockNumber(log.block_number?),
            })
        });

        if event.is_none() {
            tracing::warn!(
                client_id = %client_id,
                sequence = %sequence,
                "matched WriteAcknowledgement log but failed to decode or client mismatch"
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

                            let Ok(event) = EvmEvent::from_alloy_log(&log).inspect_err(|e| {
                                tracing::warn!("skipping malformed log: {e}");
                            }) else {
                                continue;
                            };

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

#[cfg(test)]
mod tests {
    use alloy::primitives::{Address, Bytes, LogData};
    use alloy::rpc::types::Log as RpcLog;
    use serde_json::{Value, json};

    use super::*;
    use crate::chain::test_support::{ROUTER, chain_at, no_rpc, offline_chain, spawn_rpc};
    use crate::types::LogIndex;

    fn sol_packet(source: &str, dest: &str, seq: u64) -> IICS26RouterMsgs::Packet {
        IICS26RouterMsgs::Packet {
            sequence: seq,
            sourceClient: source.to_string(),
            destClient: dest.to_string(),
            timeoutTimestamp: 1_700_000_000,
            payloads: vec![IICS26RouterMsgs::Payload {
                sourcePort: "transfer".to_string(),
                destPort: "transfer-dst".to_string(),
                version: "ics20-1".to_string(),
                encoding: "application/json".to_string(),
                value: Bytes::from_static(b"payload"),
            }],
        }
    }

    fn send_packet_data(source: &str, dest: &str, seq: u64) -> LogData {
        ICS26Router::SendPacket {
            clientId: keccak256(source),
            sequence: U256::from(seq),
            packet: sol_packet(source, dest, seq),
        }
        .encode_log_data()
    }

    fn write_ack_data(source: &str, dest: &str, seq: u64, acks: Vec<Bytes>) -> LogData {
        ICS26Router::WriteAcknowledgement {
            clientId: keccak256(dest),
            sequence: U256::from(seq),
            packet: sol_packet(source, dest, seq),
            acknowledgements: acks,
        }
        .encode_log_data()
    }

    fn evm_event(data: &LogData) -> EvmEvent {
        EvmEvent {
            address: ROUTER,
            topics: data.topics().to_vec(),
            data: data.data.to_vec(),
            block_number: BlockNumber(42),
            tx_hash: B256::repeat_byte(0xaa),
            log_index: LogIndex(0),
        }
    }

    fn rpc_log(data: LogData, block: Option<u64>) -> RpcLog {
        RpcLog {
            inner: alloy::primitives::Log {
                address: ROUTER,
                data,
            },
            block_number: block,
            transaction_hash: Some(B256::repeat_byte(0xbb)),
            log_index: Some(3),
            ..Default::default()
        }
    }

    fn logs_json(logs: &[RpcLog]) -> Value {
        serde_json::to_value(logs).unwrap()
    }

    #[test]
    fn extract_send_packet_converts_all_fields() {
        let event = evm_event(&send_packet_data("client-0", "client-1", 5));
        let send = EthereumChain::try_extract_send_packet_event(&event).unwrap();
        assert_eq!(send.block_number, BlockNumber(42));
        let p = &send.packet;
        assert_eq!(p.source_client, "client-0");
        assert_eq!(p.dest_client, "client-1");
        assert_eq!(p.sequence, PacketSequence(5));
        assert_eq!(p.timeout_timestamp, TimeoutTimestamp(1_700_000_000));
        assert_eq!(p.payloads.len(), 1);
        let pl = &p.payloads[0];
        assert_eq!(pl.source_port, Port("transfer".to_string()));
        assert_eq!(pl.dest_port, Port("transfer-dst".to_string()));
        assert_eq!(pl.version, "ics20-1");
        assert_eq!(pl.encoding, "application/json");
        assert_eq!(pl.value, b"payload");
        assert_eq!(
            EthereumChain::packet_from_send_event(&send).sequence,
            PacketSequence(5)
        );
    }

    #[test]
    fn extract_send_packet_ignores_other_events() {
        let ack = evm_event(&write_ack_data(
            "a",
            "b",
            1,
            vec![Bytes::from_static(b"ok")],
        ));
        assert!(EthereumChain::try_extract_send_packet_event(&ack).is_none());

        let mut no_topics = evm_event(&send_packet_data("a", "b", 1));
        no_topics.topics.clear();
        assert!(EthereumChain::try_extract_send_packet_event(&no_topics).is_none());
    }

    #[test]
    fn extract_send_packet_rejects_malformed_data() {
        let mut event = evm_event(&send_packet_data("a", "b", 1));
        event.data.truncate(10);
        assert!(EthereumChain::try_extract_send_packet_event(&event).is_none());

        let mut missing_indexed = evm_event(&send_packet_data("a", "b", 1));
        missing_indexed.topics.truncate(1);
        assert!(EthereumChain::try_extract_send_packet_event(&missing_indexed).is_none());
    }

    #[test]
    fn extract_write_ack_takes_first_ack() {
        let event = evm_event(&write_ack_data(
            "client-0",
            "client-1",
            9,
            vec![Bytes::from_static(b"ack-1")],
        ));
        let ack = EthereumChain::try_extract_write_ack_event(&event).unwrap();
        assert_eq!(ack.block_number, BlockNumber(42));
        assert_eq!(ack.ack.0, b"ack-1");
        assert_eq!(ack.packet.sequence, PacketSequence(9));
        assert_eq!(ack.packet.dest_client, "client-1");
        let (packet, a) = EthereumChain::packet_from_write_ack_event(&ack);
        assert_eq!(packet.sequence, PacketSequence(9));
        assert_eq!(a.0, b"ack-1");
    }

    #[test]
    fn extract_write_ack_without_acks_is_none() {
        let event = evm_event(&write_ack_data("a", "b", 1, vec![]));
        assert!(EthereumChain::try_extract_write_ack_event(&event).is_none());
    }

    #[test]
    fn extract_write_ack_ignores_other_and_malformed_events() {
        let send = evm_event(&send_packet_data("a", "b", 1));
        assert!(EthereumChain::try_extract_write_ack_event(&send).is_none());

        let mut bad = evm_event(&write_ack_data("a", "b", 1, vec![Bytes::from_static(b"x")]));
        bad.data = vec![0xff; 31];
        assert!(EthereumChain::try_extract_write_ack_event(&bad).is_none());
    }

    #[tokio::test]
    async fn query_block_events_filters_single_block() {
        let logs = logs_json(&[rpc_log(send_packet_data("a", "b", 1), Some(100))]);
        let url = spawn_rpc(move |method, params| {
            assert_eq!(method, "eth_getLogs");
            let filter = &params[0];
            assert_eq!(filter["fromBlock"], json!("0x64"));
            assert_eq!(filter["toBlock"], json!("0x64"));
            assert_eq!(
                filter["address"]
                    .as_str()
                    .unwrap()
                    .parse::<Address>()
                    .unwrap(),
                ROUTER
            );
            Ok(logs.clone())
        })
        .await;
        let events = chain_at(&url)
            .query_block_events(&EvmHeight(100))
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].block_number, BlockNumber(100));
        assert_eq!(events[0].log_index, LogIndex(3));
        assert_eq!(events[0].tx_hash, B256::repeat_byte(0xbb));
    }

    #[tokio::test]
    async fn query_block_events_fails_on_log_without_block_number() {
        let logs = logs_json(&[rpc_log(send_packet_data("a", "b", 1), None)]);
        let url = spawn_rpc(move |_, _| Ok(logs.clone())).await;
        let err = chain_at(&url)
            .query_block_events(&EvmHeight(1))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("block_number"), "{err}");
    }

    #[tokio::test]
    async fn query_block_events_propagates_rpc_error() {
        let url = spawn_rpc(no_rpc).await;
        assert!(
            chain_at(&url)
                .query_block_events(&EvmHeight(1))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn query_send_packet_event_filters_by_client_and_sequence() {
        let logs = logs_json(&[rpc_log(
            send_packet_data("client-0", "client-1", 4),
            Some(77),
        )]);
        let url = spawn_rpc(move |_, params| {
            let filter = &params[0];
            let topics = filter["topics"].as_array().unwrap();
            assert_eq!(
                topics[0].as_str().unwrap().parse::<B256>().unwrap(),
                ICS26Router::SendPacket::SIGNATURE_HASH
            );
            assert_eq!(
                topics[1].as_str().unwrap().parse::<B256>().unwrap(),
                keccak256(b"client-0")
            );
            assert_eq!(
                topics[2].as_str().unwrap().parse::<B256>().unwrap(),
                B256::from(U256::from(4))
            );
            // deployment_block from test config
            assert_eq!(filter["fromBlock"], json!("0x7"));
            Ok(logs.clone())
        })
        .await;
        let event = chain_at(&url)
            .query_send_packet_event(&EvmClientId("client-0".to_string()), PacketSequence(4))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.block_number, BlockNumber(77));
        assert_eq!(event.packet.sequence, PacketSequence(4));
    }

    #[tokio::test]
    async fn query_send_packet_event_none_when_no_logs() {
        let url = spawn_rpc(|_, _| Ok(json!([]))).await;
        let event = chain_at(&url)
            .query_send_packet_event(&EvmClientId("c".to_string()), PacketSequence(1))
            .await
            .unwrap();
        assert!(event.is_none());
    }

    #[tokio::test]
    async fn query_send_packet_event_none_when_undecodable() {
        let mut bad = send_packet_data("c", "d", 1);
        bad.data = Bytes::from_static(&[1, 2, 3]);
        let logs = logs_json(&[rpc_log(bad, Some(5))]);
        let url = spawn_rpc(move |_, _| Ok(logs.clone())).await;
        let event = chain_at(&url)
            .query_send_packet_event(&EvmClientId("c".to_string()), PacketSequence(1))
            .await
            .unwrap();
        assert!(event.is_none());
    }

    #[tokio::test]
    async fn query_send_packet_event_skips_pending_log() {
        let logs = logs_json(&[
            rpc_log(send_packet_data("c", "d", 1), None),
            rpc_log(send_packet_data("c", "d", 1), Some(9)),
        ]);
        let url = spawn_rpc(move |_, _| Ok(logs.clone())).await;
        let event = chain_at(&url)
            .query_send_packet_event(&EvmClientId("c".to_string()), PacketSequence(1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.block_number, BlockNumber(9));
    }

    #[tokio::test]
    async fn query_write_ack_event_matches_dest_client() {
        let ack = vec![Bytes::from_static(b"ack")];
        let logs = logs_json(&[
            rpc_log(
                write_ack_data("src", "other-client", 2, ack.clone()),
                Some(10),
            ),
            rpc_log(write_ack_data("src", "client-1", 2, ack), Some(11)),
        ]);
        let url = spawn_rpc(move |_, params| {
            let topics = params[0]["topics"].as_array().unwrap();
            assert_eq!(
                topics[0].as_str().unwrap().parse::<B256>().unwrap(),
                ICS26Router::WriteAcknowledgement::SIGNATURE_HASH
            );
            assert!(topics[1].is_null());
            assert_eq!(
                topics[2].as_str().unwrap().parse::<B256>().unwrap(),
                B256::from(U256::from(2))
            );
            Ok(logs.clone())
        })
        .await;
        let event = chain_at(&url)
            .query_write_ack_event(&EvmClientId("client-1".to_string()), PacketSequence(2))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.block_number, BlockNumber(11));
        assert_eq!(event.ack.0, b"ack");
    }

    #[tokio::test]
    async fn query_write_ack_event_none_on_client_mismatch_or_no_ack() {
        let logs = logs_json(&[
            rpc_log(
                write_ack_data("src", "other", 2, vec![Bytes::from_static(b"a")]),
                Some(1),
            ),
            rpc_log(write_ack_data("src", "client-1", 2, vec![]), Some(2)),
        ]);
        let url = spawn_rpc(move |_, _| Ok(logs.clone())).await;
        let event = chain_at(&url)
            .query_write_ack_event(&EvmClientId("client-1".to_string()), PacketSequence(2))
            .await
            .unwrap();
        assert!(event.is_none());
    }

    #[tokio::test]
    async fn query_write_ack_event_none_when_no_logs() {
        let url = spawn_rpc(|_, _| Ok(json!([]))).await;
        let event = chain_at(&url)
            .query_write_ack_event(&EvmClientId("c".to_string()), PacketSequence(1))
            .await
            .unwrap();
        assert!(event.is_none());
    }

    #[tokio::test]
    async fn subscribe_without_ws_addr_returns_none() {
        assert!(
            offline_chain()
                .subscribe_block_events()
                .await
                .unwrap()
                .is_none()
        );
    }
}
