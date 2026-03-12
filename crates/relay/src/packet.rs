use std::borrow::Borrow;

use async_trait::async_trait;
use tracing::debug;

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
use mercury_chain_traits::types::{
    Chain, HasChainTypes, HasIbcTypes, HasMessageTypes, HasPacketTypes, HasRevisionNumber,
};
use mercury_core::error::Result;

use crate::context::RelayContext;

#[async_trait]
impl<Src, Dst> CanBuildReceivePacketMessages for RelayContext<Src, Dst>
where
    Src: Chain<Dst> + CanQueryPacketCommitment<Dst> + HasRevisionNumber,
    Dst: Chain<Src> + CanBuildReceivePacketMessage<Src>,
    <Dst as CanBuildReceivePacketMessage<Src>>::ReceivePacketPayload: From<(
        <Src as HasIbcTypes<Dst>>::CommitmentProof,
        <Src as HasChainTypes>::Height,
        u64,
    )>,
{
    async fn build_receive_packet_messages(
        &self,
        packet: &<Src as HasPacketTypes<Dst>>::Packet,
        proof_height: &<Src as HasChainTypes>::Height,
    ) -> Result<Vec<<Dst as HasMessageTypes>::Message>> {
        let sequence = Src::packet_sequence(packet);

        let (commitment, proof) = self
            .src_chain()
            .query_packet_commitment(self.src_client_id(), sequence, proof_height)
            .await?;

        if commitment.is_none() {
            debug!(seq = sequence, "packet commitment not found, skipping");
            return Ok(vec![]);
        }

        let revision = self.src_chain().revision_number();
        let payload = (proof, proof_height.clone(), revision).into();

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
    Src: Chain<Dst> + CanQueryPacketAcknowledgement<Dst> + HasRevisionNumber,
    Dst: Chain<Src> + CanBuildAckPacketMessage<Src>,
    <Dst as CanBuildAckPacketMessage<Src>>::AckPacketPayload: From<(
        <Src as HasIbcTypes<Dst>>::CommitmentProof,
        <Src as HasChainTypes>::Height,
        u64,
    )>,
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

        let (ack_value, proof) = self
            .src_chain()
            .query_packet_acknowledgement(self.src_client_id(), sequence, proof_height)
            .await?;

        if ack_value.is_none() {
            debug!(seq = sequence, "acknowledgement not found, skipping");
            return Ok(vec![]);
        }

        let revision = self.src_chain().revision_number();
        let payload = (proof, proof_height.clone(), revision).into();

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
    Src: Chain<Dst> + CanBuildTimeoutPacketMessage<Dst>,
    Dst: Chain<Src> + CanQueryPacketReceipt<Src> + HasRevisionNumber,
    <Src as CanBuildTimeoutPacketMessage<Dst>>::TimeoutPacketPayload: From<(
        <Dst as HasIbcTypes<Src>>::CommitmentProof,
        <Dst as HasChainTypes>::Height,
        u64,
    )>,
    <Src as HasPacketTypes<Dst>>::Packet: Borrow<<Dst as HasPacketTypes<Src>>::Packet>,
{
    async fn build_timeout_packet_messages(
        &self,
        packet: &<Src as HasPacketTypes<Dst>>::Packet,
        proof_height: &<Dst as HasChainTypes>::Height,
    ) -> Result<Vec<<Src as HasMessageTypes>::Message>> {
        let sequence = Src::packet_sequence(packet);

        let (receipt, proof) = self
            .dst_chain()
            .query_packet_receipt(self.dst_client_id(), sequence, proof_height)
            .await?;

        if receipt.is_some() {
            debug!(
                seq = sequence,
                "packet already received, timeout not needed"
            );
            return Ok(vec![]);
        }

        let revision = self.dst_chain().revision_number();
        let payload = (proof, proof_height.clone(), revision).into();

        let msg = self
            .src_chain()
            .build_timeout_packet_message(packet, payload)
            .await?;

        Ok(vec![msg])
    }
}
