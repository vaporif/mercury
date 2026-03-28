use std::time::Duration;

use async_trait::async_trait;
use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder, UpdateClientOutput,
};
use mercury_chain_traits::events::PacketEvents;
use mercury_chain_traits::inner::HasCore;
use mercury_chain_traits::queries::{MisbehaviourQuery, PacketStateQuery};
use mercury_chain_traits::types::{
    ChainTypes, IbcTypes, MessageSender, PacketSequence, Port, TimeoutTimestamp, TxReceipt,
};
use mercury_core::error::Result;

use crate::CachedChain;

// No macros for easier debugging/stacktraces

impl<C: IbcTypes> ChainTypes for CachedChain<C> {
    type Height = C::Height;
    type Timestamp = C::Timestamp;
    type ChainId = C::ChainId;
    type ClientId = C::ClientId;
    type Event = C::Event;
    type Message = C::Message;
    type MessageResponse = C::MessageResponse;
    type ChainStatus = C::ChainStatus;

    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height {
        C::chain_status_height(status)
    }

    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp {
        C::chain_status_timestamp(status)
    }

    fn chain_status_timestamp_secs(status: &Self::ChainStatus) -> u64 {
        C::chain_status_timestamp_secs(status)
    }

    fn revision_number(&self) -> u64 {
        self.inner.revision_number()
    }

    fn increment_height(height: &Self::Height) -> Option<Self::Height> {
        C::increment_height(height)
    }

    fn sub_height(height: &Self::Height, n: u64) -> Option<Self::Height> {
        C::sub_height(height, n)
    }

    fn block_time(&self) -> Duration {
        self.inner.block_time()
    }

    fn chain_id(&self) -> &Self::ChainId {
        self.inner.chain_id()
    }

    fn chain_label(&self) -> mercury_core::ChainLabel {
        self.inner.chain_label()
    }
}

impl<C: IbcTypes> IbcTypes for CachedChain<C> {
    type ClientState = C::ClientState;
    type ConsensusState = C::ConsensusState;
    type CommitmentProof = C::CommitmentProof;
    type Packet = C::Packet;
    type PacketCommitment = C::PacketCommitment;
    type PacketReceipt = C::PacketReceipt;
    type Acknowledgement = C::Acknowledgement;

    fn packet_sequence(packet: &Self::Packet) -> PacketSequence {
        C::packet_sequence(packet)
    }

    fn packet_timeout_timestamp(packet: &Self::Packet) -> TimeoutTimestamp {
        C::packet_timeout_timestamp(packet)
    }

    fn packet_source_ports(packet: &Self::Packet) -> Vec<Port> {
        C::packet_source_ports(packet)
    }
}

impl<C: HasCore> HasCore for CachedChain<C> {
    type Core = C::Core;
}

#[async_trait]
impl<C: MessageSender + IbcTypes> MessageSender for CachedChain<C> {
    async fn send_messages(&self, messages: Vec<Self::Message>) -> Result<TxReceipt> {
        self.tx_handle.submit(messages).await
    }
}

#[async_trait]
impl<C: PacketStateQuery> PacketStateQuery for CachedChain<C> {
    async fn query_packet_commitment(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        height: &Self::Height,
    ) -> Result<(Option<Self::PacketCommitment>, Self::CommitmentProof)> {
        self.inner
            .query_packet_commitment(client_id, sequence, height)
            .await
    }

    async fn query_packet_receipt(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        height: &Self::Height,
    ) -> Result<(Option<Self::PacketReceipt>, Self::CommitmentProof)> {
        self.inner
            .query_packet_receipt(client_id, sequence, height)
            .await
    }

    async fn query_packet_acknowledgement(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        height: &Self::Height,
    ) -> Result<(Option<Self::Acknowledgement>, Self::CommitmentProof)> {
        self.inner
            .query_packet_acknowledgement(client_id, sequence, height)
            .await
    }

    async fn query_commitment_sequences(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Vec<PacketSequence>> {
        self.inner
            .query_commitment_sequences(client_id, height)
            .await
    }

    fn commitment_to_membership_entry(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        commitment: &Self::PacketCommitment,
        proof: &Self::CommitmentProof,
    ) -> Option<mercury_core::MembershipProofEntry> {
        self.inner
            .commitment_to_membership_entry(client_id, sequence, commitment, proof)
    }

    fn ack_to_membership_entry(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        ack: &Self::Acknowledgement,
        proof: &Self::CommitmentProof,
    ) -> Option<mercury_core::MembershipProofEntry> {
        self.inner
            .ack_to_membership_entry(client_id, sequence, ack, proof)
    }
}

#[async_trait]
impl<C: PacketEvents> PacketEvents for CachedChain<C> {
    type SendPacketEvent = C::SendPacketEvent;
    type WriteAckEvent = C::WriteAckEvent;

    fn try_extract_send_packet_event(event: &Self::Event) -> Option<Self::SendPacketEvent> {
        C::try_extract_send_packet_event(event)
    }

    fn try_extract_write_ack_event(event: &Self::Event) -> Option<Self::WriteAckEvent> {
        C::try_extract_write_ack_event(event)
    }

    fn packet_from_send_event(event: &Self::SendPacketEvent) -> &Self::Packet {
        C::packet_from_send_event(event)
    }

    fn packet_from_write_ack_event(
        event: &Self::WriteAckEvent,
    ) -> (&Self::Packet, &Self::Acknowledgement) {
        C::packet_from_write_ack_event(event)
    }

    async fn query_block_events(&self, height: &Self::Height) -> Result<Vec<Self::Event>> {
        self.inner.query_block_events(height).await
    }

    async fn query_send_packet_event(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
    ) -> Result<Option<Self::SendPacketEvent>> {
        self.inner
            .query_send_packet_event(client_id, sequence)
            .await
    }
}

#[async_trait]
impl<X: ChainTypes, C: ClientPayloadBuilder<X> + IbcTypes> ClientPayloadBuilder<X>
    for CachedChain<C>
{
    type CreateClientPayload = C::CreateClientPayload;
    type UpdateClientPayload = C::UpdateClientPayload;

    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload> {
        self.inner.build_create_client_payload().await
    }

    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
        counterparty_client_state: &<X as IbcTypes>::ClientState,
    ) -> Result<Self::UpdateClientPayload>
    where
        X: IbcTypes,
    {
        self.inner
            .build_update_client_payload(trusted_height, target_height, counterparty_client_state)
            .await
    }

    fn update_payload_proof_height(
        &self,
        payload: &Self::UpdateClientPayload,
    ) -> Option<Self::Height> {
        self.inner.update_payload_proof_height(payload)
    }
}

#[async_trait]
impl<X: ChainTypes, C: ClientMessageBuilder<X>> ClientMessageBuilder<X> for CachedChain<C> {
    type CreateClientPayload = C::CreateClientPayload;
    type UpdateClientPayload = C::UpdateClientPayload;

    async fn build_create_client_message(
        &self,
        payload: Self::CreateClientPayload,
    ) -> Result<Self::Message> {
        self.inner.build_create_client_message(payload).await
    }

    async fn build_update_client_message(
        &self,
        client_id: &Self::ClientId,
        payload: Self::UpdateClientPayload,
    ) -> Result<UpdateClientOutput<Self::Message>> {
        self.inner
            .build_update_client_message(client_id, payload)
            .await
    }

    async fn build_register_counterparty_message(
        &self,
        client_id: &Self::ClientId,
        counterparty_client_id: &X::ClientId,
        counterparty_merkle_prefix: mercury_core::MerklePrefix,
    ) -> Result<Self::Message> {
        self.inner
            .build_register_counterparty_message(
                client_id,
                counterparty_client_id,
                counterparty_merkle_prefix,
            )
            .await
    }

    fn enrich_update_payload(
        &self,
        payload: &mut Self::UpdateClientPayload,
        proofs: &[mercury_core::MembershipProofEntry],
    ) {
        self.inner.enrich_update_payload(payload, proofs);
    }

    fn finalize_batch(
        &self,
        update_output: &mut UpdateClientOutput<Self::Message>,
        packet_messages: &mut [Self::Message],
    ) {
        self.inner.finalize_batch(update_output, packet_messages);
    }
}

#[async_trait]
impl<X: ChainTypes, C: MisbehaviourDetector<X>> MisbehaviourDetector<X> for CachedChain<C> {
    type UpdateHeader = C::UpdateHeader;
    type MisbehaviourEvidence = C::MisbehaviourEvidence;
    type CounterpartyClientState = C::CounterpartyClientState;

    async fn check_for_misbehaviour(
        &self,
        client_id: &X::ClientId,
        update_header: &Self::UpdateHeader,
        client_state: &Self::CounterpartyClientState,
    ) -> Result<Option<Self::MisbehaviourEvidence>> {
        self.inner
            .check_for_misbehaviour(client_id, update_header, client_state)
            .await
    }
}

#[async_trait]
impl<X: ChainTypes, C: MisbehaviourMessageBuilder<X>> MisbehaviourMessageBuilder<X>
    for CachedChain<C>
{
    type MisbehaviourEvidence = C::MisbehaviourEvidence;

    async fn build_misbehaviour_message(
        &self,
        client_id: &Self::ClientId,
        evidence: Self::MisbehaviourEvidence,
    ) -> Result<Self::Message> {
        self.inner
            .build_misbehaviour_message(client_id, evidence)
            .await
    }
}

#[async_trait]
impl<X: ChainTypes, C: MisbehaviourQuery<X>> MisbehaviourQuery<X> for CachedChain<C> {
    type CounterpartyUpdateHeader = C::CounterpartyUpdateHeader;

    async fn query_consensus_state_heights(
        &self,
        client_id: &Self::ClientId,
    ) -> Result<Vec<X::Height>> {
        self.inner.query_consensus_state_heights(client_id).await
    }

    async fn query_update_client_header(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &X::Height,
    ) -> Result<Option<Self::CounterpartyUpdateHeader>> {
        self.inner
            .query_update_client_header(client_id, consensus_height)
            .await
    }
}

#[async_trait]
impl<X: IbcTypes, C: PacketMessageBuilder<X>> PacketMessageBuilder<X> for CachedChain<C> {
    async fn build_receive_packet_message(
        &self,
        packet: &X::Packet,
        proof: X::CommitmentProof,
        proof_height: X::Height,
        revision: u64,
    ) -> Result<Self::Message> {
        self.inner
            .build_receive_packet_message(packet, proof, proof_height, revision)
            .await
    }

    async fn build_ack_packet_message(
        &self,
        packet: &X::Packet,
        ack: &X::Acknowledgement,
        proof: X::CommitmentProof,
        proof_height: X::Height,
        revision: u64,
    ) -> Result<Self::Message> {
        self.inner
            .build_ack_packet_message(packet, ack, proof, proof_height, revision)
            .await
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &Self::Packet,
        proof: X::CommitmentProof,
        proof_height: X::Height,
        revision: u64,
    ) -> Result<Self::Message> {
        self.inner
            .build_timeout_packet_message(packet, proof, proof_height, revision)
            .await
    }
}
