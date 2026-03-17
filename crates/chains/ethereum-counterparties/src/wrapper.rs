use async_trait::async_trait;
use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder, UpdateClientOutput,
};
use mercury_chain_traits::events::PacketEvents;
use mercury_chain_traits::inner::HasInner;
use mercury_chain_traits::queries::MisbehaviourQuery;
use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use mercury_chain_traits::types::{ChainTypes, IbcTypes, MessageSender};
use mercury_core::error::Result;

use mercury_ethereum::chain::EthereumChainInner;
use mercury_ethereum::config::EthereumChainConfig;

/// Wrapper around `EthereumChainInner` that is local to this crate,
/// enabling cross-chain trait impls without orphan rule violations.
#[derive(Clone, Debug)]
pub struct EthereumChain(pub EthereumChainInner);

impl EthereumChain {
    pub async fn new(
        config: EthereumChainConfig,
        signer: alloy::signers::local::PrivateKeySigner,
    ) -> Result<Self> {
        EthereumChainInner::new(config, signer).await.map(Self)
    }
}

impl std::ops::Deref for EthereumChain {
    type Target = EthereumChainInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl HasInner for EthereumChain {
    type Inner = EthereumChainInner;
}

impl ChainTypes for EthereumChain {
    type Height = <EthereumChainInner as ChainTypes>::Height;
    type Timestamp = <EthereumChainInner as ChainTypes>::Timestamp;
    type ChainId = <EthereumChainInner as ChainTypes>::ChainId;
    type ClientId = <EthereumChainInner as ChainTypes>::ClientId;
    type Event = <EthereumChainInner as ChainTypes>::Event;
    type Message = <EthereumChainInner as ChainTypes>::Message;
    type MessageResponse = <EthereumChainInner as ChainTypes>::MessageResponse;
    type ChainStatus = <EthereumChainInner as ChainTypes>::ChainStatus;

    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height {
        EthereumChainInner::chain_status_height(status)
    }
    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp {
        EthereumChainInner::chain_status_timestamp(status)
    }
    fn chain_status_timestamp_secs(status: &Self::ChainStatus) -> u64 {
        EthereumChainInner::chain_status_timestamp_secs(status)
    }
    fn revision_number(&self) -> u64 {
        self.0.revision_number()
    }
    fn increment_height(height: &Self::Height) -> Option<Self::Height> {
        EthereumChainInner::increment_height(height)
    }
    fn sub_height(height: &Self::Height, n: u64) -> Option<Self::Height> {
        EthereumChainInner::sub_height(height, n)
    }
    fn block_time(&self) -> std::time::Duration {
        self.0.block_time()
    }
}

impl IbcTypes for EthereumChain {
    type ClientState = <EthereumChainInner as IbcTypes>::ClientState;
    type ConsensusState = <EthereumChainInner as IbcTypes>::ConsensusState;
    type CommitmentProof = <EthereumChainInner as IbcTypes>::CommitmentProof;
    type Packet = <EthereumChainInner as IbcTypes>::Packet;
    type PacketCommitment = <EthereumChainInner as IbcTypes>::PacketCommitment;
    type PacketReceipt = <EthereumChainInner as IbcTypes>::PacketReceipt;
    type Acknowledgement = <EthereumChainInner as IbcTypes>::Acknowledgement;

    fn packet_sequence(packet: &Self::Packet) -> u64 {
        EthereumChainInner::packet_sequence(packet)
    }
    fn packet_timeout_timestamp(packet: &Self::Packet) -> u64 {
        EthereumChainInner::packet_timeout_timestamp(packet)
    }
    fn packet_source_ports(packet: &Self::Packet) -> Vec<String> {
        EthereumChainInner::packet_source_ports(packet)
    }
}

#[async_trait]
impl MessageSender for EthereumChain {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<mercury_chain_traits::types::TxReceipt> {
        self.0.send_messages(messages).await
    }
}

#[async_trait]
impl ChainStatusQuery for EthereumChain {
    async fn query_chain_status(&self) -> Result<Self::ChainStatus> {
        self.0.query_chain_status().await
    }
}

#[async_trait]
impl PacketStateQuery for EthereumChain {
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
impl PacketEvents for EthereumChain {
    type SendPacketEvent = <EthereumChainInner as PacketEvents>::SendPacketEvent;
    type WriteAckEvent = <EthereumChainInner as PacketEvents>::WriteAckEvent;

    fn try_extract_send_packet_event(event: &Self::Event) -> Option<Self::SendPacketEvent> {
        EthereumChainInner::try_extract_send_packet_event(event)
    }
    fn try_extract_write_ack_event(event: &Self::Event) -> Option<Self::WriteAckEvent> {
        EthereumChainInner::try_extract_write_ack_event(event)
    }
    fn packet_from_send_event(event: &Self::SendPacketEvent) -> &Self::Packet {
        EthereumChainInner::packet_from_send_event(event)
    }
    fn packet_from_write_ack_event(
        event: &Self::WriteAckEvent,
    ) -> (&Self::Packet, &Self::Acknowledgement) {
        EthereumChainInner::packet_from_write_ack_event(event)
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
impl ClientPayloadBuilder<EthereumChainInner> for EthereumChain {
    type CreateClientPayload =
        <EthereumChainInner as ClientPayloadBuilder<EthereumChainInner>>::CreateClientPayload;
    type UpdateClientPayload =
        <EthereumChainInner as ClientPayloadBuilder<EthereumChainInner>>::UpdateClientPayload;

    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload> {
        self.0.build_create_client_payload().await
    }

    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
        counterparty_client_state: &<EthereumChainInner as IbcTypes>::ClientState,
    ) -> Result<Self::UpdateClientPayload> {
        self.0
            .build_update_client_payload(trusted_height, target_height, counterparty_client_state)
            .await
    }
}

#[async_trait]
impl ClientQuery<EthereumChainInner> for EthereumChain {
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
        EthereumChainInner::trusting_period(client_state)
    }

    fn client_latest_height(client_state: &Self::ClientState) -> Self::Height {
        EthereumChainInner::client_latest_height(client_state)
    }
}

#[async_trait]
impl ClientMessageBuilder<EthereumChainInner> for EthereumChain
where
    EthereumChainInner: ClientMessageBuilder<EthereumChainInner>,
{
    type CreateClientPayload =
        <EthereumChainInner as ClientMessageBuilder<EthereumChainInner>>::CreateClientPayload;
    type UpdateClientPayload =
        <EthereumChainInner as ClientMessageBuilder<EthereumChainInner>>::UpdateClientPayload;

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
        counterparty_client_id: &<EthereumChainInner as ChainTypes>::ClientId,
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
impl PacketMessageBuilder<EthereumChainInner> for EthereumChain
where
    EthereumChainInner: PacketMessageBuilder<EthereumChainInner>,
{
    async fn build_receive_packet_message(
        &self,
        packet: &<EthereumChainInner as IbcTypes>::Packet,
        proof: <EthereumChainInner as IbcTypes>::CommitmentProof,
        proof_height: <EthereumChainInner as ChainTypes>::Height,
        revision: u64,
    ) -> Result<Self::Message> {
        self.0
            .build_receive_packet_message(packet, proof, proof_height, revision)
            .await
    }

    async fn build_ack_packet_message(
        &self,
        packet: &<EthereumChainInner as IbcTypes>::Packet,
        ack: &<EthereumChainInner as IbcTypes>::Acknowledgement,
        proof: <EthereumChainInner as IbcTypes>::CommitmentProof,
        proof_height: <EthereumChainInner as ChainTypes>::Height,
        revision: u64,
    ) -> Result<Self::Message> {
        self.0
            .build_ack_packet_message(packet, ack, proof, proof_height, revision)
            .await
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &Self::Packet,
        proof: <EthereumChainInner as IbcTypes>::CommitmentProof,
        proof_height: <EthereumChainInner as ChainTypes>::Height,
        revision: u64,
    ) -> Result<Self::Message> {
        self.0
            .build_timeout_packet_message(packet, proof, proof_height, revision)
            .await
    }
}

#[async_trait]
impl MisbehaviourDetector<EthereumChainInner> for EthereumChain {
    type UpdateHeader = ();
    type MisbehaviourEvidence = ();
    type CounterpartyClientState = mercury_ethereum::types::EvmClientState;

    async fn check_for_misbehaviour(
        &self,
        client_id: &Self::ClientId,
        update_header: &(),
        client_state: &mercury_ethereum::types::EvmClientState,
    ) -> Result<Option<()>> {
        self.0
            .check_for_misbehaviour(client_id, update_header, client_state)
            .await
    }
}

#[async_trait]
impl MisbehaviourMessageBuilder<EthereumChainInner> for EthereumChain {
    type MisbehaviourEvidence = ();

    async fn build_misbehaviour_message(
        &self,
        client_id: &Self::ClientId,
        evidence: (),
    ) -> Result<Self::Message> {
        self.0.build_misbehaviour_message(client_id, evidence).await
    }
}

#[async_trait]
impl MisbehaviourQuery<EthereumChainInner> for EthereumChain {
    type CounterpartyUpdateHeader = ();

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
    ) -> Result<Option<()>> {
        self.0
            .query_update_client_header(client_id, consensus_height)
            .await
    }
}
