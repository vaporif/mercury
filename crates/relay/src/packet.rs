use async_trait::async_trait;
use tracing::{debug, instrument};

use crate::context::RelayContext;
use mercury_chain_traits::builders::PacketMessageBuilder;
use mercury_chain_traits::inner::HasInner;
use mercury_chain_traits::relay::{PacketBuildResult, Relay, RelayChain, RelayPacketBuilder};
use mercury_chain_traits::types::{ChainTypes, IbcTypes};

#[async_trait]
impl<Src, Dst> RelayPacketBuilder for RelayContext<Src, Dst>
where
    Src: RelayChain + PacketMessageBuilder<<Dst as HasInner>::Inner>,
    Dst: RelayChain + PacketMessageBuilder<<Src as HasInner>::Inner>,
    Self: Relay<SrcChain = Src, DstChain = Dst>,
{
    #[instrument(skip_all, name = "build_receive_packet", fields(seq = Src::packet_sequence(packet)))]
    async fn build_receive_packet_messages(
        &self,
        packet: &<Src as IbcTypes>::Packet,
        proof_height: &<Src as ChainTypes>::Height,
    ) -> PacketBuildResult<<Dst as ChainTypes>::Message> {
        let sequence = Src::packet_sequence(packet);

        let (commitment, proof) = self
            .src_chain()
            .query_packet_commitment(self.src_client_id(), sequence, proof_height)
            .await?;

        let Some(commitment) = commitment else {
            debug!(seq = sequence, "packet commitment not found, skipping");
            return Ok((vec![], vec![]));
        };

        let mut membership_entries = Vec::new();
        if let Some(entry) = self.src_chain().commitment_to_membership_entry(
            self.src_client_id(),
            sequence,
            &commitment,
            &proof,
        ) {
            membership_entries.push(entry);
        }

        let revision = self.src_chain().revision_number();

        let msg = self
            .dst_chain()
            .build_receive_packet_message(packet, proof, proof_height.clone(), revision)
            .await?;

        Ok((vec![msg], membership_entries))
    }

    #[instrument(skip_all, name = "build_ack_packet", fields(seq = Src::packet_sequence(packet)))]
    async fn build_ack_packet_messages(
        &self,
        packet: &<Src as IbcTypes>::Packet,
        ack: &<Src as IbcTypes>::Acknowledgement,
        proof_height: &<Src as ChainTypes>::Height,
    ) -> PacketBuildResult<<Dst as ChainTypes>::Message> {
        let sequence = Src::packet_sequence(packet);

        let (ack_value, proof) = self
            .src_chain()
            .query_packet_acknowledgement(self.src_client_id(), sequence, proof_height)
            .await?;

        let Some(ack_value) = ack_value else {
            debug!(seq = sequence, "acknowledgement not found, skipping");
            return Ok((vec![], vec![]));
        };

        let mut membership_entries = Vec::new();
        if let Some(entry) = self.src_chain().ack_to_membership_entry(
            self.src_client_id(),
            sequence,
            &ack_value,
            &proof,
        ) {
            membership_entries.push(entry);
        }

        let revision = self.src_chain().revision_number();

        let msg = self
            .dst_chain()
            .build_ack_packet_message(packet, ack, proof, proof_height.clone(), revision)
            .await?;

        Ok((vec![msg], membership_entries))
    }

    #[instrument(skip_all, name = "build_timeout_packet", fields(seq = Src::packet_sequence(packet)))]
    async fn build_timeout_packet_messages(
        &self,
        packet: &<Src as IbcTypes>::Packet,
        proof_height: &<Dst as ChainTypes>::Height,
    ) -> mercury_core::error::Result<Vec<<Src as ChainTypes>::Message>> {
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

        let msg = self
            .src_chain()
            .build_timeout_packet_message(packet, proof, proof_height.clone(), revision)
            .await?;

        Ok(vec![msg])
    }
}
