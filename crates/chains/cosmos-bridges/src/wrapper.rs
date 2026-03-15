use async_trait::async_trait;
use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder, UpdateClientOutput,
};
use mercury_chain_traits::events::PacketEvents;
use mercury_chain_traits::inner::HasInner;
use mercury_chain_traits::queries::{
    ChainStatusQuery, ClientQuery, MisbehaviourQuery, PacketStateQuery,
};
use mercury_chain_traits::types::{ChainTypes, IbcTypes, MessageSender};
use mercury_core::error::Result;

use mercury_cosmos::chain::CosmosChainInner;
use mercury_cosmos::config::CosmosChainConfig;
use mercury_cosmos::keys::CosmosSigner;

/// Wrapper around `CosmosChainInner` that is local to this crate,
/// enabling cross-chain trait impls without orphan rule violations.
#[derive(Clone, Debug)]
pub struct CosmosChain<S: CosmosSigner>(pub CosmosChainInner<S>);

impl<S: CosmosSigner> CosmosChain<S> {
    pub async fn new(config: CosmosChainConfig, signer: S) -> Result<Self> {
        CosmosChainInner::new(config, signer).await.map(Self)
    }
}

impl<S: CosmosSigner> std::ops::Deref for CosmosChain<S> {
    type Target = CosmosChainInner<S>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S: CosmosSigner> HasInner for CosmosChain<S> {
    type Inner = CosmosChainInner<S>;
}

impl<S: CosmosSigner> ChainTypes for CosmosChain<S> {
    type Height = <CosmosChainInner<S> as ChainTypes>::Height;
    type Timestamp = <CosmosChainInner<S> as ChainTypes>::Timestamp;
    type ChainId = <CosmosChainInner<S> as ChainTypes>::ChainId;
    type ClientId = <CosmosChainInner<S> as ChainTypes>::ClientId;
    type Event = <CosmosChainInner<S> as ChainTypes>::Event;
    type Message = <CosmosChainInner<S> as ChainTypes>::Message;
    type MessageResponse = <CosmosChainInner<S> as ChainTypes>::MessageResponse;
    type ChainStatus = <CosmosChainInner<S> as ChainTypes>::ChainStatus;

    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height {
        CosmosChainInner::<S>::chain_status_height(status)
    }
    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp {
        CosmosChainInner::<S>::chain_status_timestamp(status)
    }
    fn chain_status_timestamp_secs(status: &Self::ChainStatus) -> u64 {
        CosmosChainInner::<S>::chain_status_timestamp_secs(status)
    }
    fn revision_number(&self) -> u64 {
        self.0.revision_number()
    }
    fn increment_height(height: &Self::Height) -> Option<Self::Height> {
        CosmosChainInner::<S>::increment_height(height)
    }
    fn sub_height(height: &Self::Height, n: u64) -> Option<Self::Height> {
        CosmosChainInner::<S>::sub_height(height, n)
    }
    fn block_time(&self) -> std::time::Duration {
        self.0.block_time()
    }
}

impl<S: CosmosSigner> IbcTypes for CosmosChain<S> {
    type ClientState = <CosmosChainInner<S> as IbcTypes>::ClientState;
    type ConsensusState = <CosmosChainInner<S> as IbcTypes>::ConsensusState;
    type CommitmentProof = <CosmosChainInner<S> as IbcTypes>::CommitmentProof;
    type Packet = <CosmosChainInner<S> as IbcTypes>::Packet;
    type PacketCommitment = <CosmosChainInner<S> as IbcTypes>::PacketCommitment;
    type PacketReceipt = <CosmosChainInner<S> as IbcTypes>::PacketReceipt;
    type Acknowledgement = <CosmosChainInner<S> as IbcTypes>::Acknowledgement;

    fn packet_sequence(packet: &Self::Packet) -> u64 {
        CosmosChainInner::<S>::packet_sequence(packet)
    }
    fn packet_timeout_timestamp(packet: &Self::Packet) -> u64 {
        CosmosChainInner::<S>::packet_timeout_timestamp(packet)
    }
    fn packet_source_ports(packet: &Self::Packet) -> Vec<String> {
        CosmosChainInner::<S>::packet_source_ports(packet)
    }
}

#[async_trait]
impl<S: CosmosSigner> MessageSender for CosmosChain<S> {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<Vec<Self::MessageResponse>> {
        self.0.send_messages(messages).await
    }
}

#[async_trait]
impl<S: CosmosSigner> ChainStatusQuery for CosmosChain<S> {
    async fn query_chain_status(&self) -> Result<Self::ChainStatus> {
        self.0.query_chain_status().await
    }
}

#[async_trait]
impl<S: CosmosSigner> PacketStateQuery for CosmosChain<S> {
    async fn query_packet_commitment(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::PacketCommitment>, Self::CommitmentProof)> {
        self.0
            .query_packet_commitment(client_id, sequence, height)
            .await
    }

    async fn query_packet_receipt(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::PacketReceipt>, Self::CommitmentProof)> {
        self.0
            .query_packet_receipt(client_id, sequence, height)
            .await
    }

    async fn query_packet_acknowledgement(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::Acknowledgement>, Self::CommitmentProof)> {
        self.0
            .query_packet_acknowledgement(client_id, sequence, height)
            .await
    }

    async fn query_commitment_sequences(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Vec<u64>> {
        self.0.query_commitment_sequences(client_id, height).await
    }
}

#[async_trait]
impl<S: CosmosSigner> PacketEvents for CosmosChain<S> {
    type SendPacketEvent = <CosmosChainInner<S> as PacketEvents>::SendPacketEvent;
    type WriteAckEvent = <CosmosChainInner<S> as PacketEvents>::WriteAckEvent;

    fn try_extract_send_packet_event(event: &Self::Event) -> Option<Self::SendPacketEvent> {
        CosmosChainInner::<S>::try_extract_send_packet_event(event)
    }
    fn try_extract_write_ack_event(event: &Self::Event) -> Option<Self::WriteAckEvent> {
        CosmosChainInner::<S>::try_extract_write_ack_event(event)
    }
    fn packet_from_send_event(event: &Self::SendPacketEvent) -> &Self::Packet {
        CosmosChainInner::<S>::packet_from_send_event(event)
    }
    fn packet_from_write_ack_event(
        event: &Self::WriteAckEvent,
    ) -> (&Self::Packet, &Self::Acknowledgement) {
        CosmosChainInner::<S>::packet_from_write_ack_event(event)
    }
    async fn query_block_events(&self, height: &Self::Height) -> Result<Vec<Self::Event>> {
        self.0.query_block_events(height).await
    }
    async fn query_send_packet_event(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
    ) -> Result<Option<Self::SendPacketEvent>> {
        self.0.query_send_packet_event(client_id, sequence).await
    }
}

#[async_trait]
impl<S: CosmosSigner, C: ChainTypes> ClientPayloadBuilder<C> for CosmosChain<S>
where
    CosmosChainInner<S>: ClientPayloadBuilder<C>,
{
    type CreateClientPayload =
        <CosmosChainInner<S> as ClientPayloadBuilder<C>>::CreateClientPayload;
    type UpdateClientPayload =
        <CosmosChainInner<S> as ClientPayloadBuilder<C>>::UpdateClientPayload;

    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload> {
        self.0.build_create_client_payload().await
    }

    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
        counterparty_client_state: &<C as IbcTypes>::ClientState,
    ) -> Result<Self::UpdateClientPayload>
    where
        C: IbcTypes,
    {
        self.0
            .build_update_client_payload(trusted_height, target_height, counterparty_client_state)
            .await
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientQuery<CosmosChainInner<S>> for CosmosChain<S> {
    async fn query_client_state(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Self::ClientState> {
        self.0.query_client_state(client_id, height).await
    }

    async fn query_consensus_state(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &Self::Height,
        query_height: &Self::Height,
    ) -> Result<Self::ConsensusState> {
        self.0
            .query_consensus_state(client_id, consensus_height, query_height)
            .await
    }

    fn trusting_period(client_state: &Self::ClientState) -> Option<std::time::Duration> {
        CosmosChainInner::<S>::trusting_period(client_state)
    }

    fn client_latest_height(client_state: &Self::ClientState) -> Self::Height {
        CosmosChainInner::<S>::client_latest_height(client_state)
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientMessageBuilder<CosmosChainInner<S>> for CosmosChain<S>
where
    CosmosChainInner<S>: ClientMessageBuilder<CosmosChainInner<S>>,
{
    type CreateClientPayload =
        <CosmosChainInner<S> as ClientMessageBuilder<CosmosChainInner<S>>>::CreateClientPayload;
    type UpdateClientPayload =
        <CosmosChainInner<S> as ClientMessageBuilder<CosmosChainInner<S>>>::UpdateClientPayload;

    async fn build_create_client_message(
        &self,
        payload: Self::CreateClientPayload,
    ) -> Result<Self::Message> {
        self.0.build_create_client_message(payload).await
    }

    async fn build_update_client_message(
        &self,
        client_id: &Self::ClientId,
        payload: Self::UpdateClientPayload,
    ) -> Result<UpdateClientOutput<Self::Message>> {
        self.0.build_update_client_message(client_id, payload).await
    }

    async fn build_register_counterparty_message(
        &self,
        client_id: &Self::ClientId,
        counterparty_client_id: &<CosmosChainInner<S> as ChainTypes>::ClientId,
        counterparty_merkle_prefix: mercury_core::MerklePrefix,
    ) -> Result<Self::Message> {
        self.0
            .build_register_counterparty_message(
                client_id,
                counterparty_client_id,
                counterparty_merkle_prefix,
            )
            .await
    }
}

#[async_trait]
impl<S: CosmosSigner> PacketMessageBuilder<CosmosChainInner<S>> for CosmosChain<S>
where
    CosmosChainInner<S>: PacketMessageBuilder<CosmosChainInner<S>>,
{
    async fn build_receive_packet_message(
        &self,
        packet: &<CosmosChainInner<S> as IbcTypes>::Packet,
        proof: <CosmosChainInner<S> as IbcTypes>::CommitmentProof,
        proof_height: <CosmosChainInner<S> as ChainTypes>::Height,
        revision: u64,
    ) -> Result<Self::Message> {
        self.0
            .build_receive_packet_message(packet, proof, proof_height, revision)
            .await
    }

    async fn build_ack_packet_message(
        &self,
        packet: &<CosmosChainInner<S> as IbcTypes>::Packet,
        ack: &<CosmosChainInner<S> as IbcTypes>::Acknowledgement,
        proof: <CosmosChainInner<S> as IbcTypes>::CommitmentProof,
        proof_height: <CosmosChainInner<S> as ChainTypes>::Height,
        revision: u64,
    ) -> Result<Self::Message> {
        self.0
            .build_ack_packet_message(packet, ack, proof, proof_height, revision)
            .await
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &Self::Packet,
        proof: <CosmosChainInner<S> as IbcTypes>::CommitmentProof,
        proof_height: <CosmosChainInner<S> as ChainTypes>::Height,
        revision: u64,
    ) -> Result<Self::Message> {
        self.0
            .build_timeout_packet_message(packet, proof, proof_height, revision)
            .await
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourDetector<CosmosChainInner<S>> for CosmosChain<S>
where
    CosmosChainInner<S>: MisbehaviourDetector<CosmosChainInner<S>>,
{
    type UpdateHeader =
        <CosmosChainInner<S> as MisbehaviourDetector<CosmosChainInner<S>>>::UpdateHeader;
    type MisbehaviourEvidence =
        <CosmosChainInner<S> as MisbehaviourDetector<CosmosChainInner<S>>>::MisbehaviourEvidence;
    type CounterpartyClientState =
        <CosmosChainInner<S> as MisbehaviourDetector<CosmosChainInner<S>>>::CounterpartyClientState;

    async fn check_for_misbehaviour(
        &self,
        client_id: &<CosmosChainInner<S> as ChainTypes>::ClientId,
        update_header: &Self::UpdateHeader,
        client_state: &Self::CounterpartyClientState,
    ) -> Result<Option<Self::MisbehaviourEvidence>> {
        self.0
            .check_for_misbehaviour(client_id, update_header, client_state)
            .await
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourMessageBuilder<CosmosChainInner<S>> for CosmosChain<S>
where
    CosmosChainInner<S>: MisbehaviourMessageBuilder<CosmosChainInner<S>>,
{
    type MisbehaviourEvidence = <CosmosChainInner<S> as MisbehaviourMessageBuilder<
        CosmosChainInner<S>,
    >>::MisbehaviourEvidence;

    async fn build_misbehaviour_message(
        &self,
        client_id: &Self::ClientId,
        evidence: Self::MisbehaviourEvidence,
    ) -> Result<Self::Message> {
        self.0.build_misbehaviour_message(client_id, evidence).await
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourQuery<CosmosChainInner<S>> for CosmosChain<S>
where
    CosmosChainInner<S>: MisbehaviourQuery<CosmosChainInner<S>>,
{
    type CounterpartyUpdateHeader =
        <CosmosChainInner<S> as MisbehaviourQuery<CosmosChainInner<S>>>::CounterpartyUpdateHeader;

    async fn query_consensus_state_heights(
        &self,
        client_id: &Self::ClientId,
    ) -> Result<Vec<Self::Height>> {
        self.0.query_consensus_state_heights(client_id).await
    }

    async fn query_update_client_header(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &Self::Height,
    ) -> Result<Option<Self::CounterpartyUpdateHeader>> {
        self.0
            .query_update_client_header(client_id, consensus_height)
            .await
    }
}
