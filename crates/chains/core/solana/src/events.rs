use std::sync::LazyLock;

use borsh::BorshDeserialize;
use ibc_proto::ibc::core::channel::v2::Acknowledgement;
use prost::Message as _;

use crate::accounts::{ANCHOR_DISCRIMINATOR_LEN, anchor_discriminator};
use crate::types::{
    SendPacketEvent, SolanaAcknowledgement, SolanaEvent, SolanaPacket, SolanaPayload, WriteAckEvent,
};

const PROGRAM_DATA_PREFIX: &str = "Program data: ";

/// Borsh layout of `ics26_router::events::SendPacketEvent`.
#[derive(BorshDeserialize, Debug)]
pub struct RawSendPacketEvent {
    pub client_id: String,
    pub sequence: u64,
    pub packet: RawPacket,
    pub timeout_timestamp: u64,
}

/// Borsh layout of `ics26_router::events::WriteAcknowledgementEvent`.
#[derive(BorshDeserialize, Debug)]
pub struct RawWriteAckEvent {
    pub client_id: String,
    pub sequence: u64,
    pub packet: RawPacket,
    pub acknowledgements: Vec<Vec<u8>>,
}

#[derive(BorshDeserialize, Debug)]
pub struct RawPacket {
    pub sequence: u64,
    pub source_client: String,
    pub dest_client: String,
    pub timeout_timestamp: u64,
    pub payloads: Vec<RawPayload>,
}

#[derive(BorshDeserialize, Debug)]
pub struct RawPayload {
    pub source_port: String,
    pub dest_port: String,
    pub version: String,
    pub encoding: String,
    pub value: Vec<u8>,
}

fn event_discriminator(event_name: &str) -> [u8; ANCHOR_DISCRIMINATOR_LEN] {
    anchor_discriminator("event", event_name)
}

static SEND_PACKET_DISC: LazyLock<[u8; ANCHOR_DISCRIMINATOR_LEN]> =
    LazyLock::new(|| event_discriminator("SendPacketEvent"));
static WRITE_ACK_DISC: LazyLock<[u8; ANCHOR_DISCRIMINATOR_LEN]> =
    LazyLock::new(|| event_discriminator("WriteAcknowledgementEvent"));

fn parse_log_line(line: &str) -> Option<SolanaEvent> {
    let b64 = line.strip_prefix(PROGRAM_DATA_PREFIX)?;
    let data =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.trim()).ok()?;
    if data.len() < ANCHOR_DISCRIMINATOR_LEN {
        return None;
    }
    Some(SolanaEvent {
        program_id: String::new(),
        data,
    })
}

#[must_use]
pub fn try_decode_send_packet(event: &SolanaEvent) -> Option<SendPacketEvent> {
    if event.data.len() < ANCHOR_DISCRIMINATOR_LEN {
        return None;
    }
    let (disc, body) = event.data.split_at(ANCHOR_DISCRIMINATOR_LEN);
    if disc != *SEND_PACKET_DISC {
        return None;
    }
    let raw = RawSendPacketEvent::try_from_slice(body).ok()?;
    Some(SendPacketEvent {
        packet: raw.packet.into(),
    })
}

#[must_use]
pub fn try_decode_write_ack(event: &SolanaEvent) -> Option<WriteAckEvent> {
    if event.data.len() < ANCHOR_DISCRIMINATOR_LEN {
        return None;
    }
    let (disc, body) = event.data.split_at(ANCHOR_DISCRIMINATOR_LEN);
    if disc != *WRITE_ACK_DISC {
        return None;
    }
    let raw = RawWriteAckEvent::try_from_slice(body).ok()?;
    // Counterparties decode the ack as a proto `Acknowledgement` carrying one app ack per payload.
    let ack = Acknowledgement {
        app_acknowledgements: raw.acknowledgements,
    };
    Some(WriteAckEvent {
        packet: raw.packet.into(),
        ack: SolanaAcknowledgement(ack.encode_to_vec()),
    })
}

impl From<RawPacket> for SolanaPacket {
    fn from(raw: RawPacket) -> Self {
        Self {
            source_client_id: raw.source_client,
            dest_client_id: raw.dest_client,
            sequence: raw.sequence.into(),
            timeout_timestamp: raw.timeout_timestamp.into(),
            payloads: raw
                .payloads
                .into_iter()
                .map(|p| SolanaPayload {
                    source_port: p.source_port.into(),
                    dest_port: p.dest_port.into(),
                    version: p.version,
                    encoding: p.encoding,
                    data: p.value,
                })
                .collect(),
        }
    }
}

#[must_use]
pub fn extract_events_from_logs(logs: &[String], ics26_program_id: &str) -> Vec<SolanaEvent> {
    let invoke_msg = format!("Program {ics26_program_id} invoke");
    let success_msg = format!("Program {ics26_program_id} success");
    let failed_msg = format!("Program {ics26_program_id} failed");
    let mut depth: u32 = 0;
    let mut events = Vec::new();

    for line in logs {
        if line.starts_with(&invoke_msg) {
            depth += 1;
        } else if depth > 0 && (line.starts_with(&success_msg) || line.starts_with(&failed_msg)) {
            depth = depth.saturating_sub(1);
        } else if depth > 0
            && let Some(mut event) = parse_log_line(line)
        {
            ics26_program_id.clone_into(&mut event.program_id);
            events.push(event);
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tuples serialize field by field, mirroring the on-chain structs without reusing our decoders' types.
    type WirePayload = (String, String, String, String, Vec<u8>);
    type WirePacket = (u64, String, String, u64, Vec<WirePayload>);

    fn wire_packet() -> WirePacket {
        (
            7,
            "07-tendermint-0".into(),
            "08-wasm-0".into(),
            1_700_000_000,
            vec![(
                "transfer".into(),
                "transfer".into(),
                "1".into(),
                "json".into(),
                b"hello".to_vec(),
            )],
        )
    }

    fn event(
        disc: [u8; ANCHOR_DISCRIMINATOR_LEN],
        body: &impl borsh::BorshSerialize,
    ) -> SolanaEvent {
        let mut data = disc.to_vec();
        data.extend(borsh::to_vec(body).unwrap());
        SolanaEvent {
            program_id: String::new(),
            data,
        }
    }

    fn assert_packet(packet: &SolanaPacket) {
        assert_eq!(packet.source_client_id, "07-tendermint-0");
        assert_eq!(packet.dest_client_id, "08-wasm-0");
        assert_eq!(packet.sequence, 7.into());
        assert_eq!(packet.timeout_timestamp, 1_700_000_000.into());
        assert_eq!(packet.payloads.len(), 1);
        assert_eq!(packet.payloads[0].version, "1");
        assert_eq!(packet.payloads[0].encoding, "json");
        assert_eq!(packet.payloads[0].data, b"hello");
    }

    #[test]
    fn decodes_send_packet_event() {
        let body = (
            "07-tendermint-0".to_string(),
            7u64,
            wire_packet(),
            1_700_000_000u64,
        );
        let decoded = try_decode_send_packet(&event(*SEND_PACKET_DISC, &body)).unwrap();
        assert_packet(&decoded.packet);
    }

    #[test]
    fn decodes_write_ack_event_as_proto_acknowledgement() {
        let acks = vec![b"ack-1".to_vec(), b"ack-2".to_vec()];
        let body = ("08-wasm-0".to_string(), 7u64, wire_packet(), acks.clone());
        let decoded = try_decode_write_ack(&event(*WRITE_ACK_DISC, &body)).unwrap();
        assert_packet(&decoded.packet);
        let ack = Acknowledgement::decode(decoded.ack.0.as_slice()).unwrap();
        assert_eq!(ack.app_acknowledgements, acks);
    }

    #[test]
    fn rejects_mismatched_discriminator() {
        let body = (
            "07-tendermint-0".to_string(),
            7u64,
            wire_packet(),
            1_700_000_000u64,
        );
        assert!(try_decode_send_packet(&event(*WRITE_ACK_DISC, &body)).is_none());
    }

    #[test]
    fn rejects_truncated_body() {
        let body = ("07-tendermint-0".to_string(), 7u64);
        assert!(try_decode_send_packet(&event(*SEND_PACKET_DISC, &body)).is_none());
    }
}
