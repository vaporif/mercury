use mercury_chain_traits::events::CanExtractPacketEvents;

use crate::chain::CosmosChain;
use crate::types::{CosmosEvent, CosmosPacket, SendPacketEvent};

impl CanExtractPacketEvents<CosmosChain> for CosmosChain {
    type SendPacketEvent = SendPacketEvent;

    fn try_extract_send_packet_event(event: &CosmosEvent) -> Option<SendPacketEvent> {
        if event.kind == "send_packet" {
            // TODO: parse packet fields from event attributes
            None
        } else {
            None
        }
    }

    fn packet_from_send_event(event: &SendPacketEvent) -> &CosmosPacket {
        &event.packet
    }
}
