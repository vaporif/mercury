use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::builders::{ClientMessageBuilder, ClientPayloadBuilder, PacketMessageBuilder};
use crate::events::PacketEvents;
use crate::inner::HasCore;
use crate::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use crate::types::{ChainTypes, IbcTypes, MessageSender};

/// Common bounds shared by all chains participating in a relay.
pub trait RelayChain:
    HasCore + ChainStatusQuery + MessageSender + PacketStateQuery + PacketEvents
{
}

impl<T: HasCore + ChainStatusQuery + MessageSender + PacketStateQuery + PacketEvents> RelayChain
    for T
{
}

/// A unidirectional relay context between a source and destination chain.
pub trait Relay: ThreadSafe {
    type SrcChain: RelayChain
        + ClientPayloadBuilder<
            <Self::DstChain as HasCore>::Core,
            UpdateClientPayload = <Self::DstChain as ClientMessageBuilder<
                <Self::SrcChain as HasCore>::Core,
            >>::UpdateClientPayload,
            CreateClientPayload = <Self::DstChain as ClientMessageBuilder<
                <Self::SrcChain as HasCore>::Core,
            >>::CreateClientPayload,
        >;

    /// `ClientPayloadBuilder` is bound here (not on `SrcChain`) so the compiler can resolve
    /// the associated type equality constraints on `SrcChain::*Payload` above.
    type DstChain: RelayChain
        + ClientQuery<<Self::SrcChain as HasCore>::Core>
        + ClientMessageBuilder<<Self::SrcChain as HasCore>::Core>
        + PacketMessageBuilder<<Self::SrcChain as HasCore>::Core>
        + ClientPayloadBuilder<<Self::SrcChain as HasCore>::Core>;

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

#[async_trait]
pub trait ClientUpdater: Relay {
    async fn update_dst_client(&self) -> Result<()>;
}

pub type PacketBuildResult<M> = Result<(Vec<M>, Vec<mercury_core::MembershipProofEntry>)>;

#[async_trait]
pub trait RelayPacketBuilder: Relay {
    async fn build_receive_packet_messages(
        &self,
        packet: &<Self::SrcChain as IbcTypes>::Packet,
        proof_height: &<Self::SrcChain as ChainTypes>::Height,
    ) -> PacketBuildResult<<Self::DstChain as ChainTypes>::Message>;

    async fn build_ack_packet_messages(
        &self,
        packet: &<Self::SrcChain as IbcTypes>::Packet,
        ack: &<Self::SrcChain as IbcTypes>::Acknowledgement,
        proof_height: &<Self::SrcChain as ChainTypes>::Height,
    ) -> PacketBuildResult<<Self::DstChain as ChainTypes>::Message>;

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
