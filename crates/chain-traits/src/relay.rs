use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::builders::{ClientMessageBuilder, ClientPayloadBuilder, PacketMessageBuilder};
use crate::events::PacketEvents;
use crate::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use crate::types::{ChainTypes, IbcTypes, MessageSender};

/// A unidirectional relay context between a source and destination chain.
pub trait Relay: ThreadSafe {
    type SrcChain: ChainTypes
        + ChainStatusQuery
        + MessageSender
        + IbcTypes
        + ClientPayloadBuilder<Self::DstChain>
        + PacketEvents
        + PacketStateQuery;
    type DstChain: ChainTypes
        + ChainStatusQuery
        + MessageSender
        + IbcTypes
        + ClientMessageBuilder<
            Self::SrcChain,
            CreateClientPayload = <Self::SrcChain as ClientPayloadBuilder<Self::DstChain>>::CreateClientPayload,
            UpdateClientPayload = <Self::SrcChain as ClientPayloadBuilder<Self::DstChain>>::UpdateClientPayload,
        >
        + ClientQuery<Self::SrcChain>
        + PacketStateQuery
        + PacketMessageBuilder<Self::SrcChain>
        + ClientPayloadBuilder<Self::SrcChain>;

    fn src_chain(&self) -> &Self::SrcChain;
    fn dst_chain(&self) -> &Self::DstChain;
    fn src_client_id(&self) -> &<Self::SrcChain as ChainTypes>::ClientId;
    fn dst_client_id(&self) -> &<Self::DstChain as ChainTypes>::ClientId;
}

/// A bidirectional relay that holds both A-to-B and B-to-A relay contexts.
pub trait BiRelay: ThreadSafe {
    type RelayAToB: Relay;
    type RelayBToA: Relay<
            SrcChain = <Self::RelayAToB as Relay>::DstChain,
            DstChain = <Self::RelayAToB as Relay>::SrcChain,
        >;

    fn relay_a_to_b(&self) -> &Self::RelayAToB;
    fn relay_b_to_a(&self) -> &Self::RelayBToA;
}

/// Updates the IBC light client on the destination chain.
#[async_trait]
pub trait ClientUpdater: Relay {
    async fn update_dst_client(&self) -> Result<()>;
}

/// Builds relay-level packet messages (receive, ack, timeout).
#[async_trait]
pub trait RelayPacketBuilder: Relay {
    async fn build_receive_packet_messages(
        &self,
        packet: &<Self::SrcChain as IbcTypes>::Packet,
        proof_height: &<Self::SrcChain as ChainTypes>::Height,
    ) -> Result<Vec<<Self::DstChain as ChainTypes>::Message>>;

    async fn build_ack_packet_messages(
        &self,
        packet: &<Self::SrcChain as IbcTypes>::Packet,
        ack: &<Self::SrcChain as IbcTypes>::Acknowledgement,
        proof_height: &<Self::SrcChain as ChainTypes>::Height,
    ) -> Result<Vec<<Self::DstChain as ChainTypes>::Message>>;

    /// Build timeout messages to submit to the **source** chain.
    /// The proof of non-receipt comes from the destination chain.
    async fn build_timeout_packet_messages(
        &self,
        packet: &<Self::SrcChain as IbcTypes>::Packet,
        proof_height: &<Self::DstChain as ChainTypes>::Height,
    ) -> Result<Vec<<Self::SrcChain as ChainTypes>::Message>>;
}

/// An IBC event relevant to packet relaying.
pub enum IbcEvent<R: Relay> {
    SendPacket(<R::SrcChain as PacketEvents>::SendPacketEvent),
    WriteAck(<R::SrcChain as PacketEvents>::WriteAckEvent),
}
