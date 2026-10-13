use alloy_sol_types::{SolValue, sol};
use ibc_proto::ibc::core::channel::v2::Packet as ProtoPacket;

sol! {
    /// Mirrors `IICS26RouterMsgs.Payload`.
    struct Payload {
        string sourcePort;
        string destPort;
        string version;
        string encoding;
        bytes value;
    }

    /// Mirrors `IICS26RouterMsgs.Packet`, the encoding attestors decode.
    struct Packet {
        uint64 sequence;
        string sourceClient;
        string destClient;
        uint64 timeoutTimestamp;
        Payload[] payloads;
    }
}

/// ABI-encodes an IBC v2 packet the way attestors expect it in
/// `PacketAttestationRequest.packets`.
#[must_use]
pub fn abi_encode_packet(packet: &ProtoPacket) -> Vec<u8> {
    Packet {
        sequence: packet.sequence,
        sourceClient: packet.source_client.clone(),
        destClient: packet.destination_client.clone(),
        timeoutTimestamp: packet.timeout_timestamp,
        payloads: packet
            .payloads
            .iter()
            .map(|p| Payload {
                sourcePort: p.source_port.clone(),
                destPort: p.destination_port.clone(),
                version: p.version.clone(),
                encoding: p.encoding.clone(),
                value: p.value.clone().into(),
            })
            .collect(),
    }
    .abi_encode()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibc_proto::ibc::core::channel::v2::Payload as ProtoPayload;

    #[test]
    fn round_trips_through_abi_decode() {
        let packet = ProtoPacket {
            sequence: 7,
            source_client: "client-0".into(),
            destination_client: "08-wasm-1".into(),
            timeout_timestamp: 1_700_000_000,
            payloads: vec![ProtoPayload {
                source_port: "transfer".into(),
                destination_port: "transfer".into(),
                version: "ics20-1".into(),
                encoding: "application/json".into(),
                value: b"{}".to_vec(),
            }],
        };

        let encoded = abi_encode_packet(&packet);
        let decoded = Packet::abi_decode(&encoded);

        let decoded = decoded.ok();
        assert_eq!(decoded.as_ref().map(|p| p.sequence), Some(7));
        assert_eq!(
            decoded.as_ref().map(|p| p.destClient.as_str()),
            Some("08-wasm-1")
        );
        assert_eq!(
            decoded
                .as_ref()
                .and_then(|p| p.payloads.first())
                .map(|p| p.value.to_vec()),
            Some(b"{}".to_vec())
        );
    }
}
