use async_trait::async_trait;
use ibc::core::host::types::identifiers::ClientId;
use mercury_chain_traits::events::{CanExtractPacketEvents, CanQueryBlockEvents};
use mercury_core::error::{Error, Result};
use tendermint_rpc::Client;

use crate::chain::CosmosChain;
use crate::types::{CosmosEvent, CosmosPacket, PacketPayload, SendPacketEvent};

fn get_attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn parse_payloads(attrs: &[(String, String)]) -> Vec<PacketPayload> {
    let mut payloads = Vec::new();
    let mut idx = 0u32;
    let mut key_buf = String::with_capacity(48);
    loop {
        let prefix = format!("packet_payload_{idx}_");
        let mut attr = |suffix: &str| -> Option<&str> {
            key_buf.clear();
            key_buf.push_str(&prefix);
            key_buf.push_str(suffix);
            get_attr(attrs, &key_buf)
        };
        let source_port = attr("source_port");
        let dest_port = attr("dest_port");
        let version = attr("version");
        let encoding = attr("encoding");
        let data = attr("data");

        if let (Some(sp), Some(dp), Some(v), Some(enc), Some(d)) =
            (source_port, dest_port, version, encoding, data)
        {
            let data_bytes = hex::decode(d).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "failed to hex-decode packet payload data, using raw bytes");
                d.as_bytes().to_vec()
            });
            payloads.push(PacketPayload {
                source_port: sp.to_string(),
                dest_port: dp.to_string(),
                version: v.to_string(),
                encoding: enc.to_string(),
                data: data_bytes,
            });
            idx += 1;
        } else {
            break;
        }
    }

    // JSON fallback: try "packet_payloads" attribute
    if payloads.is_empty()
        && let Some(json_str) = get_attr(attrs, "packet_payloads")
        && let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(json_str)
    {
        for val in parsed {
            let source_port = val
                .get("source_port")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let dest_port = val
                .get("dest_port")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let version = val
                .get("version")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let encoding = val
                .get("encoding")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let data_str = val
                .get("data")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let data_bytes = hex::decode(data_str).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "failed to hex-decode packet payload data, using raw bytes");
                data_str.as_bytes().to_vec()
            });

            payloads.push(PacketPayload {
                source_port: source_port.to_string(),
                dest_port: dest_port.to_string(),
                version: version.to_string(),
                encoding: encoding.to_string(),
                data: data_bytes,
            });
        }
    }

    payloads
}

impl CanExtractPacketEvents<Self> for CosmosChain {
    type SendPacketEvent = SendPacketEvent;

    fn try_extract_send_packet_event(event: &CosmosEvent) -> Option<SendPacketEvent> {
        if event.kind != "send_packet" {
            return None;
        }

        let sequence: u64 = get_attr(&event.attributes, "packet_sequence")?
            .parse()
            .ok()?;
        let source_client_id: ClientId = get_attr(&event.attributes, "packet_src_client")?
            .parse()
            .ok()?;
        let dest_client_id: ClientId = get_attr(&event.attributes, "packet_dst_client")?
            .parse()
            .ok()?;
        let timeout_timestamp: u64 = get_attr(&event.attributes, "packet_timeout_timestamp")?
            .parse()
            .ok()?;

        let payloads = parse_payloads(&event.attributes);

        Some(SendPacketEvent {
            packet: CosmosPacket {
                source_client_id,
                dest_client_id,
                sequence,
                timeout_timestamp,
                payloads,
            },
        })
    }

    fn packet_from_send_event(event: &SendPacketEvent) -> &CosmosPacket {
        &event.packet
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
impl CanQueryBlockEvents for CosmosChain {
    async fn query_block_events(
        &self,
        height: &tendermint::block::Height,
    ) -> Result<Vec<CosmosEvent>> {
        let results = self
            .rpc_client
            .block_results(*height)
            .await
            .map_err(Error::report)?;

        let mut events = Vec::new();

        // CometBFT 0.38+: finalize_block_events
        for event in &results.finalize_block_events {
            events.push(abci_event_to_cosmos_event(event));
        }

        // Pre-0.38: begin_block_events / end_block_events
        if let Some(begin_events) = &results.begin_block_events {
            for event in begin_events {
                events.push(abci_event_to_cosmos_event(event));
            }
        }
        if let Some(end_events) = &results.end_block_events {
            for event in end_events {
                events.push(abci_event_to_cosmos_event(event));
            }
        }

        // Transaction events
        if let Some(tx_results) = &results.txs_results {
            for tx_result in tx_results {
                for event in &tx_result.events {
                    events.push(abci_event_to_cosmos_event(event));
                }
            }
        }

        Ok(events)
    }

    async fn query_latest_height(&self) -> Result<tendermint::block::Height> {
        let status = self.rpc_client.status().await.map_err(Error::report)?;
        Ok(status.sync_info.latest_block_height)
    }
}
