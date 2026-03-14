use alloy::primitives::{B256, U256, keccak256};
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use async_trait::async_trait;
use eyre::Context;
use mercury_chain_traits::events::PacketEvents;
use mercury_core::error::Result;

use crate::chain::EthereumChain;
use crate::contracts::{ICS26Router, IICS26RouterMsgs};
use crate::types::{
    EvmAcknowledgement, EvmClientId, EvmEvent, EvmHeight, EvmPacket, EvmPayload,
    EvmSendPacketEvent, EvmWriteAckEvent,
};

fn sol_packet_to_evm(p: &IICS26RouterMsgs::Packet) -> EvmPacket {
    EvmPacket {
        source_client: p.sourceClient.clone(),
        dest_client: p.destClient.clone(),
        sequence: p.sequence,
        timeout_timestamp: p.timeoutTimestamp,
        payloads: p
            .payloads
            .iter()
            .map(|pl| EvmPayload {
                source_port: pl.sourcePort.clone(),
                dest_port: pl.destPort.clone(),
                version: pl.version.clone(),
                encoding: pl.encoding.clone(),
                value: pl.value.to_vec(),
            })
            .collect(),
    }
}

#[async_trait]
impl PacketEvents<Self> for EthereumChain {
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
            .provider
            .get_logs(&filter)
            .await
            .wrap_err("querying block logs")?;

        Ok(logs.iter().map(EvmEvent::from_alloy_log).collect())
    }

    async fn query_send_packet_event(
        &self,
        client_id: &EvmClientId,
        sequence: u64,
    ) -> Result<Option<EvmSendPacketEvent>> {
        let filter = Filter::new()
            .address(self.router_address)
            .event_signature(ICS26Router::SendPacket::SIGNATURE_HASH)
            .topic1(keccak256(client_id.0.as_bytes()))
            .topic2(B256::from(U256::from(sequence)))
            .from_block(self.config.deployment_block);

        let logs = self
            .provider
            .get_logs(&filter)
            .await
            .wrap_err("querying SendPacket event")?;

        let event = logs.iter().find_map(|log| {
            let decoded = ICS26Router::SendPacket::decode_log(log.as_ref()).ok()?;
            Some(EvmSendPacketEvent {
                packet: sol_packet_to_evm(&decoded.data.packet),
                block_number: log.block_number?,
            })
        });

        if logs.is_empty() {
            return Ok(None);
        }

        if event.is_none() {
            tracing::warn!(
                client_id = %client_id,
                sequence,
                "matched SendPacket log but failed to decode"
            );
        }

        Ok(event)
    }
}
