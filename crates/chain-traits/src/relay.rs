use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::events::CanExtractPacketEvents;
use crate::types::{Chain, HasChainTypes, HasIbcTypes};

/// A unidirectional relay context between a source and destination chain.
pub trait Relay: ThreadSafe {
    type SrcChain: Chain<Self::DstChain>;
    type DstChain: Chain<Self::SrcChain>;

    fn src_chain(&self) -> &Self::SrcChain;
    fn dst_chain(&self) -> &Self::DstChain;
    fn src_client_id(&self) -> &<Self::SrcChain as HasIbcTypes<Self::DstChain>>::ClientId;
    fn dst_client_id(&self) -> &<Self::DstChain as HasIbcTypes<Self::SrcChain>>::ClientId;
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

/// Updates the IBC light clients on the source and destination chains.
#[async_trait]
pub trait CanUpdateClient: Relay {
    async fn update_src_client(&self) -> Result<()>;
    async fn update_dst_client(&self) -> Result<()>;
}

/// Builds relay-level packet messages (receive, ack, timeout).
#[async_trait]
pub trait CanBuildRelayPacketMessages: Relay {
    async fn build_receive_packet_messages(
        &self,
        packet: &<Self::SrcChain as HasIbcTypes<Self::DstChain>>::Packet,
        proof_height: &<Self::SrcChain as HasChainTypes>::Height,
    ) -> Result<Vec<<Self::DstChain as HasChainTypes>::Message>>;

    async fn build_ack_packet_messages(
        &self,
        packet: &<Self::SrcChain as HasIbcTypes<Self::DstChain>>::Packet,
        ack: &<Self::DstChain as HasIbcTypes<Self::SrcChain>>::Acknowledgement,
        proof_height: &<Self::SrcChain as HasChainTypes>::Height,
    ) -> Result<Vec<<Self::DstChain as HasChainTypes>::Message>>;

    /// Build timeout messages to submit to the **source** chain.
    /// The proof of non-receipt comes from the destination chain.
    async fn build_timeout_packet_messages(
        &self,
        packet: &<Self::SrcChain as HasIbcTypes<Self::DstChain>>::Packet,
        proof_height: &<Self::DstChain as HasChainTypes>::Height,
    ) -> Result<Vec<<Self::SrcChain as HasChainTypes>::Message>>;
}

/// An IBC event relevant to packet relaying.
pub enum IbcEvent<R: Relay> {
    SendPacket(<R::SrcChain as CanExtractPacketEvents<R::DstChain>>::SendPacketEvent),
    WriteAck(<R::SrcChain as CanExtractPacketEvents<R::DstChain>>::WriteAckEvent),
}
