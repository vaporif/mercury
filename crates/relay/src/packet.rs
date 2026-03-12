use std::borrow::Borrow;

use async_trait::async_trait;

use mercury_chain_traits::events::CanExtractPacketEvents;
use mercury_chain_traits::messaging::CanSendMessages;
use mercury_chain_traits::packet_builders::{
    CanBuildAckPacketMessage, CanBuildReceivePacketMessage, CanBuildTimeoutPacketMessage,
};
use mercury_chain_traits::packet_queries::{
    CanQueryPacketAcknowledgement, CanQueryPacketCommitment, CanQueryPacketReceipt,
};
use mercury_chain_traits::relay::context::Relay;
use mercury_chain_traits::relay::packet::{
    CanBuildAckPacketMessages, CanBuildReceivePacketMessages, CanBuildTimeoutPacketMessages,
};
use mercury_chain_traits::types::{HasChainTypes, HasIbcTypes, HasMessageTypes, HasPacketTypes};
use mercury_core::error::Result;

use crate::context::RelayContext;

#[async_trait]
impl<Src, Dst> CanBuildReceivePacketMessages for RelayContext<Src, Dst>
where
    Src: HasMessageTypes
        + HasIbcTypes<Dst>
        + HasPacketTypes<Dst>
        + CanSendMessages
        + CanExtractPacketEvents<Dst>
        + CanQueryPacketCommitment<Dst>,
    Dst: HasMessageTypes
        + HasIbcTypes<Src>
        + HasPacketTypes<Src>
        + CanSendMessages
        + CanExtractPacketEvents<Src>
        + CanBuildReceivePacketMessage<Src>,
    <Dst as CanBuildReceivePacketMessage<Src>>::ReceivePacketPayload:
        From<(<Src as HasIbcTypes<Dst>>::CommitmentProof, <Src as HasChainTypes>::Height)>,
{
    async fn build_receive_packet_messages(
        &self,
        packet: &<Src as HasPacketTypes<Dst>>::Packet,
        proof_height: &<Src as HasChainTypes>::Height,
    ) -> Result<Vec<<Dst as HasMessageTypes>::Message>> {
        let sequence = Src::packet_sequence(packet);

        let (_commitment, proof) = self
            .src_chain()
            .query_packet_commitment(self.src_client_id(), sequence, proof_height)
            .await?;

        let payload = (proof, proof_height.clone()).into();

        let msg = self
            .dst_chain()
            .build_receive_packet_message(packet, payload)
            .await?;

        Ok(vec![msg])
    }
}

#[async_trait]
impl<Src, Dst> CanBuildAckPacketMessages for RelayContext<Src, Dst>
where
    Src: HasMessageTypes
        + HasIbcTypes<Dst>
        + HasPacketTypes<Dst>
        + CanSendMessages
        + CanExtractPacketEvents<Dst>
        + CanQueryPacketAcknowledgement<Dst>,
    Dst: HasMessageTypes
        + HasIbcTypes<Src>
        + HasPacketTypes<Src>
        + CanSendMessages
        + CanExtractPacketEvents<Src>
        + CanBuildAckPacketMessage<Src>,
    <Dst as CanBuildAckPacketMessage<Src>>::AckPacketPayload:
        From<(<Src as HasIbcTypes<Dst>>::CommitmentProof, <Src as HasChainTypes>::Height)>,
    <Src as HasPacketTypes<Dst>>::Packet: Borrow<<Dst as HasPacketTypes<Src>>::Packet>,
    <Dst as HasPacketTypes<Src>>::Acknowledgement:
        Borrow<<Src as HasPacketTypes<Dst>>::Acknowledgement>,
{
    async fn build_ack_packet_messages(
        &self,
        packet: &<Src as HasPacketTypes<Dst>>::Packet,
        ack: &<Dst as HasPacketTypes<Src>>::Acknowledgement,
        proof_height: &<Src as HasChainTypes>::Height,
    ) -> Result<Vec<<Dst as HasMessageTypes>::Message>> {
        let sequence = Src::packet_sequence(packet);

        let (_ack_value, proof) = self
            .src_chain()
            .query_packet_acknowledgement(self.src_client_id(), sequence, proof_height)
            .await?;

        let payload = (proof, proof_height.clone()).into();

        let msg = self
            .dst_chain()
            .build_ack_packet_message(packet.borrow(), ack.borrow(), payload)
            .await?;

        Ok(vec![msg])
    }
}

#[async_trait]
impl<Src, Dst> CanBuildTimeoutPacketMessages for RelayContext<Src, Dst>
where
    Src: HasMessageTypes
        + HasIbcTypes<Dst>
        + HasPacketTypes<Dst>
        + CanSendMessages
        + CanExtractPacketEvents<Dst>,
    Dst: HasMessageTypes
        + HasIbcTypes<Src>
        + HasPacketTypes<Src>
        + CanSendMessages
        + CanExtractPacketEvents<Src>
        + CanQueryPacketReceipt<Src>
        + CanBuildTimeoutPacketMessage<Src>,
    <Dst as CanBuildTimeoutPacketMessage<Src>>::TimeoutPacketPayload:
        From<(<Dst as HasIbcTypes<Src>>::CommitmentProof, <Dst as HasChainTypes>::Height)>,
    <Src as HasPacketTypes<Dst>>::Packet: Borrow<<Dst as HasPacketTypes<Src>>::Packet>,
{
    async fn build_timeout_packet_messages(
        &self,
        packet: &<Src as HasPacketTypes<Dst>>::Packet,
        proof_height: &<Dst as HasChainTypes>::Height,
    ) -> Result<Vec<<Dst as HasMessageTypes>::Message>> {
        let sequence = Src::packet_sequence(packet);

        let (_receipt, proof) = self
            .dst_chain()
            .query_packet_receipt(self.dst_client_id(), sequence, proof_height)
            .await?;

        let payload = (proof, proof_height.clone()).into();

        let msg = self
            .dst_chain()
            .build_timeout_packet_message(packet.borrow(), payload)
            .await?;

        Ok(vec![msg])
    }
}
