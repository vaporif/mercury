use async_trait::async_trait;
use futures::{Stream, StreamExt};
use mercury_chain_traits::events::{BlockEvents, PacketEvents};
use mercury_core::error::{Result, WrapErr as _};
use prost::Message as _;
use tendermint_rpc::event::EventData;
use tendermint_rpc::query::EventType;
use tendermint_rpc::{Client, SubscriptionClient, WebSocketClient};
use tracing::{instrument, warn};

use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};

pub(crate) const ENCODED_PACKET_HEX: &str = "encoded_packet_hex";

use crate::chain::CosmosChain;
use crate::keys::CosmosSigner;
use crate::types::{
    CosmosEvent, CosmosPacket, PacketAcknowledgement, PacketPayload, SendPacketEvent, WriteAckEvent,
};
use ibc_proto::ibc::core::channel::v2 as channel;

fn get_attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn v2_packet_to_cosmos(pkt: channel::Packet) -> CosmosPacket {
    CosmosPacket {
        source_client_id: pkt.source_client.into(),
        dest_client_id: pkt.destination_client.into(),
        sequence: PacketSequence(pkt.sequence),
        timeout_timestamp: TimeoutTimestamp(pkt.timeout_timestamp),
        payloads: pkt
            .payloads
            .into_iter()
            .map(|p| PacketPayload {
                source_port: Port(p.source_port),
                dest_port: Port(p.destination_port),
                version: p.version,
                encoding: p.encoding,
                data: p.value,
            })
            .collect(),
    }
}

fn abci_event_to_cosmos_event(event: &tendermint::abci::Event) -> CosmosEvent {
    let attributes = event
        .attributes
        .iter()
        .filter_map(|attr| {
            let key = attr
                .key_str()
                .inspect_err(|e| warn!(error = %e, "event attribute key decode failed"))
                .ok()?
                .to_string();
            let value = attr
                .value_str()
                .inspect_err(|e| warn!(error = %e, "event attribute value decode failed"))
                .ok()?
                .to_string();
            Some((key, value))
        })
        .collect();

    CosmosEvent {
        kind: event.kind.clone(),
        attributes,
    }
}

#[async_trait]
impl<S: CosmosSigner> PacketEvents for CosmosChain<S> {
    type SendPacketEvent = SendPacketEvent;
    type WriteAckEvent = WriteAckEvent;

    fn try_extract_send_packet_event(event: &CosmosEvent) -> Option<SendPacketEvent> {
        if event.kind != "send_packet" {
            return None;
        }
        let hex_str = get_attr(&event.attributes, ENCODED_PACKET_HEX)?;
        let bytes = hex::decode(hex_str)
            .inspect_err(|e| warn!(error = %e, "failed to hex-decode send_packet"))
            .ok()?;
        let pkt = channel::Packet::decode(bytes.as_slice())
            .inspect_err(|e| warn!(error = %e, "failed to proto-decode send_packet"))
            .ok()?;
        Some(SendPacketEvent {
            packet: v2_packet_to_cosmos(pkt),
        })
    }

    fn try_extract_write_ack_event(event: &CosmosEvent) -> Option<WriteAckEvent> {
        if event.kind != "write_acknowledgement" {
            return None;
        }
        let pkt_hex = get_attr(&event.attributes, ENCODED_PACKET_HEX)?;
        let ack_hex = get_attr(&event.attributes, "encoded_acknowledgement_hex")?;
        let pkt_bytes = hex::decode(pkt_hex)
            .inspect_err(|e| warn!(error = %e, "failed to hex-decode write_ack packet"))
            .ok()?;
        let ack_bytes = hex::decode(ack_hex)
            .inspect_err(|e| warn!(error = %e, "failed to hex-decode write_ack acknowledgement"))
            .ok()?;
        let pkt = channel::Packet::decode(pkt_bytes.as_slice())
            .inspect_err(|e| warn!(error = %e, "failed to proto-decode write_ack packet"))
            .ok()?;
        Some(WriteAckEvent {
            packet: v2_packet_to_cosmos(pkt),
            ack: PacketAcknowledgement(ack_bytes),
        })
    }

    fn packet_from_send_event(event: &SendPacketEvent) -> &CosmosPacket {
        &event.packet
    }

    fn packet_from_write_ack_event(
        event: &WriteAckEvent,
    ) -> (&CosmosPacket, &PacketAcknowledgement) {
        (&event.packet, &event.ack)
    }

    async fn query_block_events(
        &self,
        height: &tendermint::block::Height,
    ) -> Result<Vec<CosmosEvent>> {
        let results = self
            .rpc_guard
            .guarded(|| async {
                self.rpc_client
                    .block_results(*height)
                    .await
                    .map_err(Into::into)
            })
            .await?;

        let events = results
            .finalize_block_events
            .iter()
            // Pre-0.38: begin_block_events / end_block_events
            .chain(results.begin_block_events.iter().flatten())
            .chain(results.end_block_events.iter().flatten())
            // Transaction events
            .chain(
                results
                    .txs_results
                    .iter()
                    .flatten()
                    .flat_map(|tx| &tx.events),
            )
            .map(abci_event_to_cosmos_event)
            .collect();

        Ok(events)
    }

    #[instrument(skip_all, fields(seq = %sequence))]
    async fn query_send_packet_event(
        &self,
        client_id: &ibc::core::host::types::identifiers::ClientId,
        sequence: PacketSequence,
    ) -> Result<Option<SendPacketEvent>> {
        use tendermint_rpc::query::{EventType, Query};

        let query = Query::from(EventType::Tx)
            .and_eq("send_packet.packet_sequence", sequence.0.to_string());

        let response = self
            .rpc_guard
            .guarded(|| async {
                self.rpc_client
                    .tx_search(
                        query.clone(),
                        false,
                        1,
                        100,
                        tendermint_rpc::Order::Descending,
                    )
                    .await
                    .map_err(Into::into)
            })
            .await?;

        let found = response
            .txs
            .iter()
            .flat_map(|tx| &tx.tx_result.events)
            .map(abci_event_to_cosmos_event)
            .find_map(|event| {
                let send = <Self as PacketEvents>::try_extract_send_packet_event(&event)?;
                (send.packet.source_client_id.as_ref() == client_id.as_str()).then_some(send)
            });

        if let Some(send_event) = found {
            return Ok(Some(send_event));
        }

        if response.txs.is_empty() {
            warn!(
                sequence = sequence.0,
                %client_id,
                "tx_search returned no results — event may have been pruned from node's tx index"
            );
        }

        Ok(None)
    }

    #[instrument(skip_all, fields(seq = %sequence))]
    async fn query_write_ack_event(
        &self,
        client_id: &ibc::core::host::types::identifiers::ClientId,
        sequence: PacketSequence,
    ) -> Result<Option<WriteAckEvent>> {
        use tendermint_rpc::query::{EventType, Query};

        let query = Query::from(EventType::Tx).and_eq(
            "write_acknowledgement.packet_sequence",
            sequence.0.to_string(),
        );

        let response = self
            .rpc_guard
            .guarded(|| async {
                self.rpc_client
                    .tx_search(
                        query.clone(),
                        false,
                        1,
                        100,
                        tendermint_rpc::Order::Descending,
                    )
                    .await
                    .map_err(Into::into)
            })
            .await?;

        let found = response
            .txs
            .iter()
            .flat_map(|tx| &tx.tx_result.events)
            .map(abci_event_to_cosmos_event)
            .find_map(|event| {
                let ack = <Self as PacketEvents>::try_extract_write_ack_event(&event)?;
                (ack.packet.dest_client_id.as_ref() == client_id.as_str()).then_some(ack)
            });

        if let Some(write_ack) = found {
            return Ok(Some(write_ack));
        }

        if response.txs.is_empty() {
            warn!(
                sequence = sequence.0,
                %client_id,
                "tx_search returned no results — event may have been pruned from node's tx index"
            );
        }

        Ok(None)
    }

    async fn subscribe_block_events(
        &self,
    ) -> Result<
        Option<
            mercury_chain_traits::events::BlockEventStream<tendermint::block::Height, CosmosEvent>,
        >,
    > {
        let Some(ws_addr) = &self.config.ws_addr else {
            return Ok(None);
        };

        let (client, driver) = WebSocketClient::new(ws_addr.as_str())
            .await
            .wrap_err("websocket connect failed")?;

        tokio::spawn(driver.run());

        let subscription = client
            .subscribe(tendermint_rpc::query::Query::from(EventType::Tx))
            .await
            .wrap_err("websocket subscription failed")?;

        let flush_timeout = self.config.block_time * 2;
        Ok(Some(Box::pin(cosmos_ws_stream(
            subscription,
            client,
            flush_timeout,
        ))))
    }
}

fn cosmos_ws_stream(
    subscription: tendermint_rpc::Subscription,
    client: WebSocketClient,
    flush_timeout: std::time::Duration,
) -> impl Stream<Item = Result<BlockEvents<tendermint::block::Height, CosmosEvent>>> {
    type State = (
        tendermint_rpc::Subscription,
        WebSocketClient,
        Option<(tendermint::block::Height, Vec<CosmosEvent>)>,
        std::time::Duration,
        Option<eyre::Report>,
    );

    futures::stream::unfold(
        (subscription, client, None, flush_timeout, None) as State,
        |(mut sub, client, mut pending, flush_timeout, deferred_err)| async move {
            if let Some(err) = deferred_err {
                return Some((Err(err), (sub, client, None, flush_timeout, None)));
            }

            let flush = |pending: (tendermint::block::Height, Vec<CosmosEvent>), state: State| {
                let flushed = BlockEvents {
                    height: pending.0,
                    events: pending.1,
                };
                Some((Ok(flushed), state))
            };

            loop {
                let next = tokio::time::timeout(flush_timeout, sub.next()).await;

                match next {
                    Ok(Some(Ok(event))) => {
                        let EventData::Tx { tx_result } = &event.data else {
                            continue;
                        };

                        let height = tendermint::block::Height::try_from(tx_result.height)
                            .unwrap_or_default();
                        let events: Vec<CosmosEvent> = tx_result
                            .result
                            .events
                            .iter()
                            .map(abci_event_to_cosmos_event)
                            .collect();

                        match &mut pending {
                            Some((pending_height, pending_events)) if *pending_height == height => {
                                pending_events.extend(events);
                            }
                            Some((pending_height, pending_events)) => {
                                let flushed = BlockEvents {
                                    height: *pending_height,
                                    events: std::mem::take(pending_events),
                                };
                                *pending_height = height;
                                *pending_events = events;
                                return Some((
                                    Ok(flushed),
                                    (sub, client, pending, flush_timeout, None),
                                ));
                            }
                            None => {
                                pending = Some((height, events));
                            }
                        }
                    }
                    Ok(Some(Err(e))) => {
                        let err = eyre::eyre!("websocket stream error: {e}");
                        if let Some(p) = pending.take() {
                            return flush(p, (sub, client, None, flush_timeout, Some(err)));
                        }
                        return Some((Err(err), (sub, client, None, flush_timeout, None)));
                    }
                    Ok(None) => {
                        return pending
                            .take()
                            .and_then(|p| flush(p, (sub, client, None, flush_timeout, None)));
                    }
                    Err(_timeout) => {
                        if let Some(p) = pending.take() {
                            return flush(p, (sub, client, None, flush_timeout, None));
                        }
                    }
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mercury_chain_traits::events::PacketEvents;
    use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};
    use prost::Message;

    use crate::keys::Secp256k1KeyPair;
    use ibc_proto::ibc::core::channel::v2::{Acknowledgement, Packet, Payload};

    type TestChain = CosmosChain<Secp256k1KeyPair>;

    fn test_payload(port: &str, version: &str, encoding: &str, data: &[u8]) -> Payload {
        Payload {
            source_port: port.to_string(),
            destination_port: port.to_string(),
            version: version.to_string(),
            encoding: encoding.to_string(),
            value: data.to_vec(),
        }
    }

    fn test_packet(seq: u64, timeout: u64, payloads: Vec<Payload>) -> Packet {
        Packet {
            sequence: seq,
            source_client: "07-tendermint-0".to_string(),
            destination_client: "07-tendermint-1".to_string(),
            timeout_timestamp: timeout,
            payloads,
        }
    }

    #[test]
    fn get_attr_finds_existing_key() {
        let attrs = vec![
            ("foo".to_string(), "bar".to_string()),
            ("baz".to_string(), "qux".to_string()),
        ];
        assert_eq!(get_attr(&attrs, "foo"), Some("bar"));
        assert_eq!(get_attr(&attrs, "baz"), Some("qux"));
    }

    #[test]
    fn get_attr_returns_none_for_missing_key() {
        let attrs = vec![("foo".to_string(), "bar".to_string())];
        assert_eq!(get_attr(&attrs, "missing"), None);
    }

    #[test]
    fn get_attr_empty_attrs() {
        let attrs: Vec<(String, String)> = vec![];
        assert_eq!(get_attr(&attrs, "any"), None);
    }

    #[test]
    fn v2_packet_to_cosmos_valid_packet() {
        let pkt = test_packet(
            42,
            1_700_000_000,
            vec![test_payload(
                "transfer",
                "ics20-1",
                "application/json",
                b"hello",
            )],
        );

        let result = v2_packet_to_cosmos(pkt);
        assert_eq!(result.sequence, PacketSequence(42));
        assert_eq!(result.source_client_id.as_ref(), "07-tendermint-0");
        assert_eq!(result.dest_client_id.as_ref(), "07-tendermint-1");
        assert_eq!(result.timeout_timestamp, TimeoutTimestamp(1_700_000_000));
        assert_eq!(result.payloads.len(), 1);
        assert_eq!(result.payloads[0].source_port, Port("transfer".to_string()));
        assert_eq!(result.payloads[0].data, b"hello");
    }

    #[test]
    fn try_extract_send_packet_event_wrong_kind() {
        let event = CosmosEvent {
            kind: "transfer".to_string(),
            attributes: vec![],
        };
        assert!(TestChain::try_extract_send_packet_event(&event).is_none());
    }

    #[test]
    fn try_extract_send_packet_event_missing_hex() {
        let event = CosmosEvent {
            kind: "send_packet".to_string(),
            attributes: vec![],
        };
        assert!(TestChain::try_extract_send_packet_event(&event).is_none());
    }

    #[test]
    fn try_extract_send_packet_event_valid() {
        let packet = test_packet(
            7,
            999,
            vec![test_payload("transfer", "ics20-1", "json", b"data")],
        );
        let hex_encoded = hex::encode(packet.encode_to_vec());

        let event = CosmosEvent {
            kind: "send_packet".to_string(),
            attributes: vec![(ENCODED_PACKET_HEX.to_string(), hex_encoded)],
        };

        let result = TestChain::try_extract_send_packet_event(&event);
        assert!(result.is_some());
        let send_event = result.unwrap();
        assert_eq!(send_event.packet.sequence, PacketSequence(7));
    }

    #[test]
    fn try_extract_write_ack_event_wrong_kind() {
        let event = CosmosEvent {
            kind: "send_packet".to_string(),
            attributes: vec![],
        };
        assert!(TestChain::try_extract_write_ack_event(&event).is_none());
    }

    #[test]
    fn try_extract_write_ack_event_valid() {
        let packet = test_packet(
            3,
            500,
            vec![test_payload("transfer", "ics20-1", "json", b"payload")],
        );
        let ack = Acknowledgement {
            app_acknowledgements: vec![b"ack_data".to_vec()],
        };

        let event = CosmosEvent {
            kind: "write_acknowledgement".to_string(),
            attributes: vec![
                (
                    ENCODED_PACKET_HEX.to_string(),
                    hex::encode(packet.encode_to_vec()),
                ),
                (
                    "encoded_acknowledgement_hex".to_string(),
                    hex::encode(ack.encode_to_vec()),
                ),
            ],
        };

        let result = TestChain::try_extract_write_ack_event(&event);
        assert!(result.is_some());
        let write_ack = result.unwrap();
        assert_eq!(write_ack.packet.sequence, PacketSequence(3));
        assert_eq!(write_ack.ack.0, ack.encode_to_vec());
    }

    fn send_event(attrs: Vec<(&str, &str)>) -> CosmosEvent {
        CosmosEvent {
            kind: "send_packet".to_string(),
            attributes: attrs
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn valid_packet_hex() -> String {
        hex::encode(test_packet(1, 1, vec![]).encode_to_vec())
    }

    #[test]
    fn v2_packet_to_cosmos_keeps_source_and_dest_ports_distinct() {
        let pkt = test_packet(
            1,
            0,
            vec![
                Payload {
                    source_port: "src-a".into(),
                    destination_port: "dst-a".into(),
                    version: "v1".into(),
                    encoding: "proto".into(),
                    value: vec![1],
                },
                Payload {
                    source_port: "src-b".into(),
                    destination_port: "dst-b".into(),
                    version: "v2".into(),
                    encoding: "json".into(),
                    value: vec![2],
                },
            ],
        );
        let result = v2_packet_to_cosmos(pkt);
        assert_eq!(result.payloads.len(), 2);
        assert_eq!(result.payloads[0].source_port, Port("src-a".into()));
        assert_eq!(result.payloads[0].dest_port, Port("dst-a".into()));
        assert_eq!(result.payloads[0].version, "v1");
        assert_eq!(result.payloads[0].encoding, "proto");
        assert_eq!(result.payloads[1].source_port, Port("src-b".into()));
        assert_eq!(result.payloads[1].dest_port, Port("dst-b".into()));
        assert_eq!(result.payloads[1].data, vec![2]);
    }

    #[test]
    fn try_extract_send_packet_event_invalid_hex() {
        let event = send_event(vec![(ENCODED_PACKET_HEX, "zz-not-hex")]);
        assert!(TestChain::try_extract_send_packet_event(&event).is_none());
    }

    #[test]
    fn try_extract_send_packet_event_invalid_proto() {
        // valid hex, but 0xFF is not a valid protobuf tag/wire type
        let event = send_event(vec![(ENCODED_PACKET_HEX, "ffffffff")]);
        assert!(TestChain::try_extract_send_packet_event(&event).is_none());
    }

    #[test]
    fn try_extract_send_packet_event_ignores_unrelated_attrs() {
        let hex = valid_packet_hex();
        let event = send_event(vec![
            ("packet_sequence", "1"),
            ("msg_index", "0"),
            (ENCODED_PACKET_HEX, &hex),
        ]);
        let send = TestChain::try_extract_send_packet_event(&event).unwrap();
        assert_eq!(
            TestChain::packet_from_send_event(&send).sequence,
            PacketSequence(1)
        );
    }

    fn write_ack_event(attrs: Vec<(&str, &str)>) -> CosmosEvent {
        CosmosEvent {
            kind: "write_acknowledgement".to_string(),
            ..send_event(attrs)
        }
    }

    #[test]
    fn try_extract_write_ack_event_missing_ack_attr() {
        let hex = valid_packet_hex();
        let event = write_ack_event(vec![(ENCODED_PACKET_HEX, &hex)]);
        assert!(TestChain::try_extract_write_ack_event(&event).is_none());
    }

    #[test]
    fn try_extract_write_ack_event_missing_packet_attr() {
        let event = write_ack_event(vec![("encoded_acknowledgement_hex", "01")]);
        assert!(TestChain::try_extract_write_ack_event(&event).is_none());
    }

    #[test]
    fn try_extract_write_ack_event_invalid_ack_hex() {
        let hex = valid_packet_hex();
        let event = write_ack_event(vec![
            (ENCODED_PACKET_HEX, &hex),
            ("encoded_acknowledgement_hex", "xyz"),
        ]);
        assert!(TestChain::try_extract_write_ack_event(&event).is_none());
    }

    #[test]
    fn try_extract_write_ack_event_invalid_packet_proto() {
        let event = write_ack_event(vec![
            (ENCODED_PACKET_HEX, "ffffffff"),
            ("encoded_acknowledgement_hex", "01"),
        ]);
        assert!(TestChain::try_extract_write_ack_event(&event).is_none());
    }

    #[test]
    fn try_extract_write_ack_event_keeps_ack_bytes_raw() {
        // ack bytes are passed through verbatim, not proto-decoded
        let hex = valid_packet_hex();
        let event = write_ack_event(vec![
            (ENCODED_PACKET_HEX, &hex),
            ("encoded_acknowledgement_hex", "deadbeef"),
        ]);
        let write_ack = TestChain::try_extract_write_ack_event(&event).unwrap();
        let (packet, ack) = TestChain::packet_from_write_ack_event(&write_ack);
        assert_eq!(packet.sequence, PacketSequence(1));
        assert_eq!(ack.0, vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn abci_event_to_cosmos_event_converts_v037_attributes() {
        let event = tendermint::abci::Event::new(
            "send_packet",
            [("packet_sequence", "5"), (ENCODED_PACKET_HEX, "abcd")],
        );
        let converted = abci_event_to_cosmos_event(&event);
        assert_eq!(converted.kind, "send_packet");
        assert_eq!(
            converted.attributes,
            vec![
                ("packet_sequence".to_string(), "5".to_string()),
                (ENCODED_PACKET_HEX.to_string(), "abcd".to_string()),
            ]
        );
    }

    #[test]
    fn abci_event_to_cosmos_event_drops_non_utf8_v034_attributes() {
        use tendermint::abci::{EventAttribute, v0_34};

        let attr = |key: &[u8], value: &[u8]| {
            EventAttribute::V034(v0_34::EventAttribute {
                key: key.to_vec(),
                value: value.to_vec(),
                index: true,
            })
        };
        let event = tendermint::abci::Event {
            kind: "write_acknowledgement".to_string(),
            attributes: vec![
                attr(b"good", b"value"),
                attr(&[0xff, 0xfe], b"bad-key"),
                attr(b"bad-value", &[0xc3, 0x28]),
            ],
        };
        let converted = abci_event_to_cosmos_event(&event);
        assert_eq!(
            converted.attributes,
            vec![("good".to_string(), "value".to_string())]
        );
    }

    #[tokio::test]
    async fn subscribe_block_events_without_ws_addr_returns_none() {
        let chain = crate::chain::test_support::test_chain();
        assert!(chain.config.ws_addr.is_none());
        assert!(chain.subscribe_block_events().await.unwrap().is_none());
    }
}
