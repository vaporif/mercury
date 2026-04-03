use std::sync::LazyLock;

use borsh::BorshDeserialize;

use crate::accounts::{ANCHOR_DISCRIMINATOR_LEN, anchor_discriminator};
use crate::types::{
    SendPacketEvent, SolanaAcknowledgement, SolanaEvent, SolanaPacket, SolanaPayload, WriteAckEvent,
};

const PROGRAM_DATA_PREFIX: &str = "Program data: ";

#[derive(BorshDeserialize, Debug)]
pub struct RawSendPacketEvent {
    pub source_client: String,
    pub dest_client: String,
    pub sequence: u64,
    pub timeout_timestamp: u64,
    pub payloads: Vec<RawPayload>,
}

#[derive(BorshDeserialize, Debug)]
pub struct RawPayload {
    pub source_port: String,
    pub dest_port: String,
    pub version: String,
    pub encoding: String,
    pub data: Vec<u8>,
}

#[derive(BorshDeserialize, Debug)]
pub struct RawWriteAckEvent {
    pub source_client: String,
    pub dest_client: String,
    pub sequence: u64,
    pub timeout_timestamp: u64,
    pub payloads: Vec<RawPayload>,
    pub acknowledgement: Vec<u8>,
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
        packet: raw_to_packet(
            &raw.source_client,
            &raw.dest_client,
            raw.sequence,
            raw.timeout_timestamp,
            &raw.payloads,
        ),
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
    Some(WriteAckEvent {
        packet: raw_to_packet(
            &raw.source_client,
            &raw.dest_client,
            raw.sequence,
            raw.timeout_timestamp,
            &raw.payloads,
        ),
        ack: SolanaAcknowledgement(raw.acknowledgement),
    })
}

fn raw_to_packet(
    source_client: &str,
    dest_client: &str,
    sequence: u64,
    timeout_timestamp: u64,
    payloads: &[RawPayload],
) -> SolanaPacket {
    SolanaPacket {
        source_client_id: source_client.to_owned(),
        dest_client_id: dest_client.to_owned(),
        sequence: sequence.into(),
        timeout_timestamp: timeout_timestamp.into(),
        payloads: payloads
            .iter()
            .map(|p| SolanaPayload {
                source_port: p.source_port.clone().into(),
                dest_port: p.dest_port.clone().into(),
                version: p.version.clone(),
                encoding: p.encoding.clone(),
                data: p.data.clone(),
            })
            .collect(),
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
