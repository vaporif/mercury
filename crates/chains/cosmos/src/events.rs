use async_trait::async_trait;
use mercury_chain_traits::events::{CanExtractPacketEvents, CanQueryBlockEvents};
use mercury_core::error::Result;
use prost::Message as _;
use tendermint_rpc::Client;

use crate::chain::CosmosChain;
use crate::ibc_v2::channel;
use crate::keys::CosmosSigner;
use crate::types::{
    CosmosEvent, CosmosPacket, PacketAcknowledgement, PacketPayload, SendPacketEvent, WriteAckEvent,
};

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

impl<S: CosmosSigner> CanExtractPacketEvents<Self> for CosmosChain<S> {
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
impl<S: CosmosSigner> CanQueryBlockEvents for CosmosChain<S> {
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

    async fn query_latest_height(&self) -> Result<tendermint::block::Height> {
        let status = self.rpc_client.status().await?;
        Ok(status.sync_info.latest_block_height)
    }

    fn increment_height(height: &tendermint::block::Height) -> Option<tendermint::block::Height> {
        height
            .value()
            .checked_add(1)
            .and_then(|v| tendermint::block::Height::try_from(v).ok())
    }
}
