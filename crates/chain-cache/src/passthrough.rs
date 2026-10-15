use std::time::Duration;

use async_trait::async_trait;
use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder, UpdateClientOutput, UpgradeClientPayload,
};
use mercury_chain_traits::events::{BlockEventStream, PacketEvents};
use mercury_chain_traits::inner::HasCore;
use mercury_chain_traits::queries::{MisbehaviourQuery, PacketStateQuery};
use mercury_chain_traits::types::{
    ChainTypes, IbcTypes, MessageSender, PacketSequence, Port, TimeoutTimestamp, TxReceipt,
};
use mercury_core::error::Result;

use crate::CachedChain;

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

    fn max_clock_drift(&self) -> Duration {
        self.inner.max_clock_drift()
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

    async fn query_ack_sequences(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Vec<PacketSequence>> {
        self.inner.query_ack_sequences(client_id, height).await
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

    async fn query_write_ack_event(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
    ) -> Result<Option<Self::WriteAckEvent>> {
        self.inner.query_write_ack_event(client_id, sequence).await
    }

    async fn subscribe_block_events(
        &self,
    ) -> Result<Option<BlockEventStream<Self::Height, Self::Event>>> {
        self.inner.subscribe_block_events().await
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

    fn update_payload_message_height(
        &self,
        payload: &Self::UpdateClientPayload,
    ) -> Option<Self::Height> {
        self.inner.update_payload_message_height(payload)
    }

    fn proof_height_without_update(
        &self,
        src_height: &Self::Height,
        trusted_height: &Self::Height,
    ) -> Self::Height {
        self.inner
            .proof_height_without_update(src_height, trusted_height)
    }

    fn required_dst_timestamp_secs(&self, payload: &Self::UpdateClientPayload) -> Option<u64> {
        self.inner.required_dst_timestamp_secs(payload)
    }

    async fn build_upgrade_client_payload(&self) -> Result<Option<UpgradeClientPayload>> {
        self.inner.build_upgrade_client_payload().await
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

    async fn build_upgrade_client_message(
        &self,
        client_id: &Self::ClientId,
        payload: UpgradeClientPayload,
    ) -> Result<Vec<Self::Message>> {
        self.inner
            .build_upgrade_client_message(client_id, payload)
            .await
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

#[cfg(test)]
mod tests {
    use mercury_core::{MembershipProofEntry, MerklePrefix};

    use super::*;
    use crate::testhelpers::Probe;

    type Cached = CachedChain<Probe>;

    fn cached() -> (Cached, Probe) {
        let probe = Probe::new();
        (CachedChain::new(probe.clone()), probe)
    }

    fn client() -> String {
        "client-0".to_owned()
    }

    #[tokio::test]
    async fn chain_and_ibc_types_delegate_to_inner() {
        let (chain, probe) = cached();
        let status = (5, 6);

        assert_eq!(*Cached::chain_status_height(&status), 5);
        assert_eq!(*Cached::chain_status_timestamp(&status), 6);
        assert_eq!(Cached::chain_status_timestamp_secs(&status), 60);
        assert_eq!(Cached::increment_height(&u64::MAX), None);
        assert_eq!(Cached::sub_height(&10, 3), Some(7));
        assert_eq!(chain.revision_number(), 7);
        assert_eq!(chain.block_time(), Duration::from_secs(2));
        assert_eq!(chain.chain_id(), "probe-1");
        assert_eq!(chain.chain_label().to_string(), "probe");

        assert_eq!(Cached::packet_sequence(&4), PacketSequence(4));
        assert_eq!(Cached::packet_timeout_timestamp(&4), TimeoutTimestamp(40));
        assert_eq!(Cached::packet_source_ports(&4), vec![Port("port-4".into())]);

        assert_eq!(probe.calls(), vec!["revision_number"]);
    }

    #[tokio::test]
    async fn methods_with_trait_defaults_delegate_to_inner() {
        use futures::StreamExt;

        let (chain, probe) = cached();

        assert_eq!(chain.max_clock_drift(), Duration::from_secs(9));

        let mut stream = chain.subscribe_block_events().await.unwrap().unwrap();
        let block = stream.next().await.unwrap().unwrap();
        assert_eq!((block.height, block.events), (4, vec![8]));

        let payload = ClientPayloadBuilder::<Probe>::build_upgrade_client_payload(&chain)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(payload.plan_height, 42);

        let msgs =
            ClientMessageBuilder::<Probe>::build_upgrade_client_message(&chain, &client(), payload)
                .await
                .unwrap();
        assert_eq!(msgs, vec![42]);

        assert_eq!(
            probe.calls(),
            vec![
                "subscribe_block_events",
                "build_upgrade_client_payload",
                "build_upgrade_client_message",
            ]
        );
    }

    #[tokio::test]
    async fn send_messages_goes_through_coordinator_to_inner() {
        let (chain, probe) = cached();
        let receipt = chain.send_messages(vec![1, 2]).await.unwrap();
        assert_eq!(receipt.gas_used, Some(21));
        assert_eq!(probe.state.lock().unwrap().sent, vec![vec![1, 2]]);
    }

    #[tokio::test]
    async fn send_messages_surfaces_inner_error() {
        let (chain, probe) = cached();
        probe.state.lock().unwrap().fail_sends = true;
        let err = chain.send_messages(vec![1]).await.unwrap_err();
        assert!(err.to_string().contains("broadcast rejected"));
    }

    #[tokio::test]
    async fn packet_state_queries_delegate_with_arguments() {
        let (chain, probe) = cached();
        let c = client();
        let seq = PacketSequence(3);

        assert_eq!(
            chain.query_packet_commitment(&c, seq, &9).await.unwrap(),
            (Some(3), 9)
        );
        assert_eq!(
            chain.query_packet_receipt(&c, seq, &9).await.unwrap(),
            (Some(4), 9)
        );
        assert_eq!(
            chain
                .query_packet_acknowledgement(&c, seq, &9)
                .await
                .unwrap(),
            (Some(5), 9)
        );
        assert_eq!(
            chain.query_commitment_sequences(&c, &8).await.unwrap(),
            vec![PacketSequence(8)]
        );
        assert_eq!(
            chain.query_ack_sequences(&c, &8).await.unwrap(),
            vec![PacketSequence(9)]
        );
        let commit_entry = chain
            .commitment_to_membership_entry(&c, seq, &0, &0)
            .unwrap();
        let ack_entry = chain.ack_to_membership_entry(&c, seq, &0, &0).unwrap();
        assert_eq!(
            (commit_entry.path, ack_entry.path),
            (vec![vec![1]], vec![vec![2]])
        );

        assert_eq!(
            probe.calls(),
            vec![
                "query_packet_commitment",
                "query_packet_receipt",
                "query_packet_acknowledgement",
                "query_commitment_sequences",
                "query_ack_sequences",
                "commitment_to_membership_entry",
                "ack_to_membership_entry",
            ]
        );
    }

    #[tokio::test]
    async fn packet_events_delegate() {
        let (chain, probe) = cached();
        let c = client();

        assert_eq!(Cached::try_extract_send_packet_event(&4), Some(4));
        assert_eq!(Cached::try_extract_send_packet_event(&5), None);
        assert_eq!(Cached::try_extract_write_ack_event(&5), Some((5, 105)));
        assert_eq!(Cached::try_extract_write_ack_event(&4), None);
        assert_eq!(*Cached::packet_from_send_event(&4), 4);
        assert_eq!(Cached::packet_from_write_ack_event(&(5, 105)), (&5, &105));

        assert_eq!(chain.query_block_events(&10).await.unwrap(), vec![10, 11]);
        assert_eq!(
            chain
                .query_send_packet_event(&c, PacketSequence(2))
                .await
                .unwrap(),
            Some(2)
        );
        assert_eq!(
            chain
                .query_write_ack_event(&c, PacketSequence(3))
                .await
                .unwrap(),
            Some((3, 0))
        );
        assert_eq!(
            probe.calls(),
            vec![
                "query_block_events",
                "query_send_packet_event",
                "query_write_ack_event",
            ]
        );
    }

    #[tokio::test]
    async fn client_payload_builder_delegates() {
        let (chain, probe) = cached();

        assert_eq!(
            ClientPayloadBuilder::<Probe>::build_create_client_payload(&chain)
                .await
                .unwrap(),
            11
        );
        assert_eq!(
            ClientPayloadBuilder::<Probe>::build_update_client_payload(&chain, &1, &2, &3)
                .await
                .unwrap(),
            6
        );
        assert_eq!(
            ClientPayloadBuilder::<Probe>::update_payload_proof_height(&chain, &6),
            Some(7)
        );
        assert_eq!(
            ClientPayloadBuilder::<Probe>::update_payload_message_height(&chain, &6),
            Some(8)
        );
        assert_eq!(
            ClientPayloadBuilder::<Probe>::proof_height_without_update(&chain, &50, &40),
            40
        );
        assert_eq!(
            ClientPayloadBuilder::<Probe>::required_dst_timestamp_secs(&chain, &6),
            Some(18)
        );
        assert_eq!(probe.calls().len(), 6);
    }

    #[tokio::test]
    async fn client_message_builder_delegates() {
        let (chain, probe) = cached();
        let c = client();

        assert_eq!(
            ClientMessageBuilder::<Probe>::build_create_client_message(&chain, 4)
                .await
                .unwrap(),
            5
        );
        let mut output = ClientMessageBuilder::<Probe>::build_update_client_message(&chain, &c, 9)
            .await
            .unwrap();
        assert_eq!(output.messages, vec![9]);
        assert_eq!(
            ClientMessageBuilder::<Probe>::build_register_counterparty_message(
                &chain,
                &c,
                &"abc".to_owned(),
                MerklePrefix::ibc_default(),
            )
            .await
            .unwrap(),
            5
        );

        let mut payload = 1;
        let proofs = vec![
            MembershipProofEntry {
                path: vec![],
                value: vec![],
                proof: vec![],
            };
            2
        ];
        ClientMessageBuilder::<Probe>::enrich_update_payload(&chain, &mut payload, &proofs);
        assert_eq!(payload, 3);

        let mut packets = [1, 2];
        ClientMessageBuilder::<Probe>::finalize_batch(&chain, &mut output, &mut packets);
        assert_eq!(packets, [10, 20]);
        assert_eq!(output.membership_proof, Some(vec![2]));

        assert_eq!(
            probe.calls(),
            vec![
                "build_create_client_message",
                "build_update_client_message",
                "build_register_counterparty_message",
                "enrich_update_payload",
                "finalize_batch",
            ]
        );
    }

    #[tokio::test]
    async fn misbehaviour_traits_delegate() {
        let (chain, probe) = cached();
        let c = client();

        assert_eq!(
            MisbehaviourDetector::<Probe>::check_for_misbehaviour(&chain, &c, &4, &5)
                .await
                .unwrap(),
            Some(9)
        );
        assert_eq!(
            MisbehaviourMessageBuilder::<Probe>::build_misbehaviour_message(&chain, &c, 9)
                .await
                .unwrap(),
            18
        );
        assert_eq!(
            MisbehaviourQuery::<Probe>::query_consensus_state_heights(&chain, &c)
                .await
                .unwrap(),
            vec![3, 2, 1]
        );
        assert_eq!(
            MisbehaviourQuery::<Probe>::query_update_client_header(&chain, &c, &3)
                .await
                .unwrap(),
            Some(53)
        );
        assert_eq!(probe.calls().len(), 4);
    }

    #[tokio::test]
    async fn packet_message_builder_delegates() {
        let (chain, probe) = cached();

        assert_eq!(
            PacketMessageBuilder::<Probe>::build_receive_packet_message(&chain, &1, 2, 3, 4)
                .await
                .unwrap(),
            10
        );
        assert_eq!(
            PacketMessageBuilder::<Probe>::build_ack_packet_message(&chain, &1, &2, 3, 4, 5)
                .await
                .unwrap(),
            15
        );
        assert_eq!(
            PacketMessageBuilder::<Probe>::build_timeout_packet_message(&chain, &1, 2, 3, 4)
                .await
                .unwrap(),
            1009
        );
        assert_eq!(
            probe.calls(),
            vec![
                "build_receive_packet_message",
                "build_ack_packet_message",
                "build_timeout_packet_message",
            ]
        );
    }
}
