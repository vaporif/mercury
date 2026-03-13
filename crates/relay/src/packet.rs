use std::borrow::Borrow;

use async_trait::async_trait;
use tracing::{debug, instrument};

use mercury_chain_traits::prelude::*;
use mercury_chain_traits::relay::{RelayPacketBuilder, Relay};
use mercury_core::error::Result;

use crate::context::RelayContext;

#[async_trait]
impl<Src, Dst> RelayPacketBuilder for RelayContext<Src, Dst>
where
    Src: Chain<Dst>,
    Dst: Chain<Src>,
    <Dst as PacketMessageBuilder<Src>>::ReceivePacketPayload: From<(
        <Src as IbcTypes<Dst>>::CommitmentProof,
        <Src as ChainTypes>::Height,
        u64,
    )>,
    <Dst as PacketMessageBuilder<Src>>::AckPacketPayload: From<(
        <Src as IbcTypes<Dst>>::CommitmentProof,
        <Src as ChainTypes>::Height,
        u64,
    )>,
    <Src as PacketMessageBuilder<Dst>>::TimeoutPacketPayload: From<(
        <Dst as IbcTypes<Src>>::CommitmentProof,
        <Dst as ChainTypes>::Height,
        u64,
    )>,
    <Src as IbcTypes<Dst>>::Packet: Borrow<<Dst as IbcTypes<Src>>::Packet>,
    <Dst as IbcTypes<Src>>::Acknowledgement: Borrow<<Src as IbcTypes<Dst>>::Acknowledgement>,
{
    #[instrument(skip_all, name = "build_receive_packet", fields(seq = Src::packet_sequence(packet)))]
    async fn build_receive_packet_messages(
        &self,
        packet: &<Src as IbcTypes<Dst>>::Packet,
        proof_height: &<Src as ChainTypes>::Height,
    ) -> Result<Vec<<Dst as ChainTypes>::Message>> {
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

    #[instrument(skip_all, name = "build_ack_packet", fields(seq = Src::packet_sequence(packet)))]
    async fn build_ack_packet_messages(
        &self,
        packet: &<Src as IbcTypes<Dst>>::Packet,
        ack: &<Dst as IbcTypes<Src>>::Acknowledgement,
        proof_height: &<Src as ChainTypes>::Height,
    ) -> Result<Vec<<Dst as ChainTypes>::Message>> {
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

    #[instrument(skip_all, name = "build_timeout_packet", fields(seq = Src::packet_sequence(packet)))]
    async fn build_timeout_packet_messages(
        &self,
        packet: &<Src as IbcTypes<Dst>>::Packet,
        proof_height: &<Dst as ChainTypes>::Height,
    ) -> Result<Vec<<Src as ChainTypes>::Message>> {
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
