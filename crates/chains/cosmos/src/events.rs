use async_trait::async_trait;
use mercury_chain_traits::events::PacketEvents;
use mercury_core::error::Result;
use prost::Message as _;
use tendermint_rpc::Client;
use tracing::warn;

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

fn v2_packet_to_cosmos(pkt: channel::Packet) -> Option<CosmosPacket> {
    let source_client_id = pkt.source_client.parse().ok()?;
    let dest_client_id = pkt.destination_client.parse().ok()?;
    Some(CosmosPacket {
        source_client_id,
        dest_client_id,
        sequence: pkt.sequence,
        timeout_timestamp: pkt.timeout_timestamp,
        payloads: pkt
            .payloads
            .into_iter()
            .map(|p| PacketPayload {
                source_port: p.source_port,
                dest_port: p.destination_port,
                version: p.version,
                encoding: p.encoding,
                data: p.value,
            })
            .collect(),
    })
}

fn abci_event_to_cosmos_event(event: &tendermint::abci::Event) -> CosmosEvent {
    let attributes = event
        .attributes
        .iter()
        .filter_map(|attr| {
            let key = attr.key_str().ok()?.to_string();
            let value = attr.value_str().ok()?.to_string();
            Some((key, value))
        })
        .collect();

    CosmosEvent {
        kind: event.kind.clone(),
        attributes,
    }
}

#[async_trait]
impl<S: CosmosSigner> PacketEvents<Self> for CosmosChain<S> {
    type SendPacketEvent = SendPacketEvent;
    type WriteAckEvent = WriteAckEvent;

    fn try_extract_send_packet_event(event: &CosmosEvent) -> Option<SendPacketEvent> {
        if event.kind != "send_packet" {
            return None;
        }
        let hex_str = get_attr(&event.attributes, "encoded_packet_hex")?;
        let bytes = hex::decode(hex_str).ok()?;
        let pkt = channel::Packet::decode(bytes.as_slice()).ok()?;
        Some(SendPacketEvent {
            packet: v2_packet_to_cosmos(pkt)?,
        })
    }

    fn try_extract_write_ack_event(event: &CosmosEvent) -> Option<WriteAckEvent> {
        if event.kind != "write_acknowledgement" {
            return None;
        }
        let pkt_hex = get_attr(&event.attributes, "encoded_packet_hex")?;
        let ack_hex = get_attr(&event.attributes, "encoded_acknowledgement_hex")?;
        let pkt_bytes = hex::decode(pkt_hex).ok()?;
        let ack_bytes = hex::decode(ack_hex).ok()?;
        let pkt = channel::Packet::decode(pkt_bytes.as_slice()).ok()?;
        Some(WriteAckEvent {
            packet: v2_packet_to_cosmos(pkt)?,
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
        let results = self.rpc_client.block_results(*height).await?;

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

    async fn query_send_packet_event(
        &self,
        client_id: &ibc::core::host::types::identifiers::ClientId,
        sequence: u64,
    ) -> Result<Option<SendPacketEvent>> {
        use tendermint_rpc::query::{EventType, Query};

        let query =
            Query::from(EventType::Tx).and_eq("send_packet.packet_sequence", sequence.to_string());

        let response = self
            .rpc_client
            .tx_search(query, false, 1, 100, tendermint_rpc::Order::Descending)
            .await?;

        for tx in &response.txs {
            for event in &tx.tx_result.events {
                let cosmos_event = abci_event_to_cosmos_event(event);
                if let Some(send_event) =
                    <Self as PacketEvents<Self>>::try_extract_send_packet_event(&cosmos_event)
                    && send_event.packet.source_client_id.as_str() == client_id.as_str()
                {
                    return Ok(Some(send_event));
                }
            }
        }

        if response.txs.is_empty() {
            warn!(
                sequence,
                %client_id,
                "tx_search returned no results — event may have been pruned from node's tx index"
            );
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mercury_chain_traits::events::PacketEvents;
    use prost::Message;

    use crate::keys::Secp256k1KeyPair;
    use ibc_proto::ibc::core::channel::v2::{Acknowledgement, Packet, Payload};

    type TestChain = CosmosChain<Secp256k1KeyPair>;

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
        let pkt = Packet {
            sequence: 42,
            source_client: "07-tendermint-0".to_string(),
            destination_client: "07-tendermint-1".to_string(),
            timeout_timestamp: 1_700_000_000,
            payloads: vec![Payload {
                source_port: "transfer".to_string(),
                destination_port: "transfer".to_string(),
                version: "ics20-1".to_string(),
                encoding: "application/json".to_string(),
                value: b"hello".to_vec(),
            }],
        };

        let result = v2_packet_to_cosmos(pkt).unwrap();
        assert_eq!(result.sequence, 42);
        assert_eq!(result.source_client_id.as_str(), "07-tendermint-0");
        assert_eq!(result.dest_client_id.as_str(), "07-tendermint-1");
        assert_eq!(result.timeout_timestamp, 1_700_000_000);
        assert_eq!(result.payloads.len(), 1);
        assert_eq!(result.payloads[0].source_port, "transfer");
        assert_eq!(result.payloads[0].data, b"hello");
    }

    #[test]
    fn v2_packet_to_cosmos_invalid_client_id() {
        let pkt = Packet {
            sequence: 1,
            source_client: "not a valid client id!!!".to_string(),
            destination_client: "07-tendermint-1".to_string(),
            timeout_timestamp: 0,
            payloads: vec![],
        };

        assert!(v2_packet_to_cosmos(pkt).is_none());
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
        let packet = Packet {
            sequence: 7,
            source_client: "07-tendermint-0".to_string(),
            destination_client: "07-tendermint-1".to_string(),
            timeout_timestamp: 999,
            payloads: vec![Payload {
                source_port: "transfer".to_string(),
                destination_port: "transfer".to_string(),
                version: "ics20-1".to_string(),
                encoding: "json".to_string(),
                value: b"data".to_vec(),
            }],
        };
        let hex_encoded = hex::encode(packet.encode_to_vec());

        let event = CosmosEvent {
            kind: "send_packet".to_string(),
            attributes: vec![("encoded_packet_hex".to_string(), hex_encoded)],
        };

        let result = TestChain::try_extract_send_packet_event(&event);
        assert!(result.is_some());
        let send_event = result.unwrap();
        assert_eq!(send_event.packet.sequence, 7);
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
        let packet = Packet {
            sequence: 3,
            source_client: "07-tendermint-0".to_string(),
            destination_client: "07-tendermint-1".to_string(),
            timeout_timestamp: 500,
            payloads: vec![Payload {
                source_port: "transfer".to_string(),
                destination_port: "transfer".to_string(),
                version: "ics20-1".to_string(),
                encoding: "json".to_string(),
                value: b"payload".to_vec(),
            }],
        };
        let ack = Acknowledgement {
            app_acknowledgements: vec![b"ack_data".to_vec()],
        };

        let event = CosmosEvent {
            kind: "write_acknowledgement".to_string(),
            attributes: vec![
                (
                    "encoded_packet_hex".to_string(),
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
        assert_eq!(write_ack.packet.sequence, 3);
        assert_eq!(write_ack.ack.0, ack.encode_to_vec());
    }
}
