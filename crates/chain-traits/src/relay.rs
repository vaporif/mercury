use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::builders::{ClientMessageBuilder, ClientPayloadBuilder, PacketMessageBuilder};
use crate::events::PacketEvents;
use crate::inner::{Core, HasCore};
use crate::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use crate::types::{ChainTypes, IbcTypes, MessageSender};

pub trait RelayChain:
    HasCore + ChainStatusQuery + MessageSender + PacketStateQuery + PacketEvents
{
}

impl<T: HasCore + ChainStatusQuery + MessageSender + PacketStateQuery + PacketEvents> RelayChain
    for T
{
}

pub type SrcCore<R> = Core<<R as Relay>::SrcChain>;
pub type DstCore<R> = Core<<R as Relay>::DstChain>;

pub trait Relay: ThreadSafe {
    type SrcChain: RelayChain
        + ClientPayloadBuilder<
            Core<Self::DstChain>,
            UpdateClientPayload = <Self::DstChain as ClientMessageBuilder<
                Core<Self::SrcChain>,
            >>::UpdateClientPayload,
            CreateClientPayload = <Self::DstChain as ClientMessageBuilder<
                Core<Self::SrcChain>,
            >>::CreateClientPayload,
        >;

    /// `ClientPayloadBuilder` is bound here (not on `SrcChain`) so the compiler can resolve
    /// the associated type equality constraints on `SrcChain::*Payload` above.
    type DstChain: RelayChain
        + ClientQuery<Core<Self::SrcChain>>
        + ClientMessageBuilder<Core<Self::SrcChain>>
        + PacketMessageBuilder<Core<Self::SrcChain>>
        + ClientPayloadBuilder<Core<Self::SrcChain>>;

    fn src_chain(&self) -> &Self::SrcChain;
    fn dst_chain(&self) -> &Self::DstChain;
    fn src_client_id(&self) -> &<Self::SrcChain as ChainTypes>::ClientId;
    fn dst_client_id(&self) -> &<Self::DstChain as ChainTypes>::ClientId;
}

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
    /// Build receive packet messages. `proof_height` queries proofs on the
    /// source chain; `message_proof_height` overrides the height in the IBC
    /// message sent to the destination (e.g. beacon slot vs execution block).
    async fn build_receive_packet_messages(
        &self,
        packet: &<Self::SrcChain as IbcTypes>::Packet,
        proof_height: &<Self::SrcChain as ChainTypes>::Height,
        message_proof_height: Option<&<Self::SrcChain as ChainTypes>::Height>,
    ) -> PacketBuildResult<<Self::DstChain as ChainTypes>::Message>;

    async fn build_ack_packet_messages(
        &self,
        packet: &<Self::SrcChain as IbcTypes>::Packet,
        ack: &<Self::SrcChain as IbcTypes>::Acknowledgement,
        proof_height: &<Self::SrcChain as ChainTypes>::Height,
        message_proof_height: Option<&<Self::SrcChain as ChainTypes>::Height>,
    ) -> PacketBuildResult<<Self::DstChain as ChainTypes>::Message>;

    async fn build_timeout_packet_messages(
        &self,
        packet: &<Self::SrcChain as IbcTypes>::Packet,
        proof_height: &<Self::DstChain as ChainTypes>::Height,
    ) -> Result<Vec<<Self::SrcChain as ChainTypes>::Message>>;
}

pub enum IbcEvent<R: Relay> {
    SendPacket(<R::SrcChain as PacketEvents>::SendPacketEvent),
    WriteAck(<R::SrcChain as PacketEvents>::WriteAckEvent),
}
