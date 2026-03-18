use std::time::Duration;

use async_trait::async_trait;
use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder, UpdateClientOutput,
};
use mercury_chain_traits::events::PacketEvents;
use mercury_chain_traits::queries::{
    ChainStatusQuery, ClientQuery, MisbehaviourQuery, PacketStateQuery,
};
use mercury_chain_traits::types::{
    ChainTypes, IbcTypes, MessageSender, PacketSequence, Port, TimeoutTimestamp, TxReceipt,
};
use mercury_core::error::Result;

use crate::config::SolanaChainConfig;
use crate::types::{
    SendPacketEvent, SolanaAcknowledgement, SolanaChainId, SolanaChainStatus, SolanaClientId,
    SolanaClientState, SolanaCommitmentProof, SolanaConsensusState, SolanaCreateClientPayload,
    SolanaEvent, SolanaHeight, SolanaMessage, SolanaPacket, SolanaPacketCommitment,
    SolanaPacketReceipt, SolanaTimestamp, SolanaTxResponse, SolanaUpdateClientPayload,
    WriteAckEvent,
};

#[derive(Clone, Debug)]
pub struct SolanaChain {
    pub config: SolanaChainConfig,
    label: mercury_core::ChainLabel,
}

impl SolanaChain {
    pub fn new(config: SolanaChainConfig) -> Result<Self> {
        let name = config.chain_name.as_deref().unwrap_or("solana");
        let label = mercury_core::ChainLabel::new(name);
        Ok(Self { config, label })
    }
}

impl ChainTypes for SolanaChain {
    type Height = SolanaHeight;
    type Timestamp = SolanaTimestamp;
    type ChainId = SolanaChainId;
    type ClientId = SolanaClientId;
    type Event = SolanaEvent;
    type Message = SolanaMessage;
    type MessageResponse = SolanaTxResponse;
    type ChainStatus = SolanaChainStatus;

    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height {
        &status.height
    }

    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp {
        &status.timestamp
    }

    fn chain_status_timestamp_secs(status: &Self::ChainStatus) -> u64 {
        status.timestamp.0
    }

    fn revision_number(&self) -> u64 {
        0
    }

    fn increment_height(height: &SolanaHeight) -> Option<SolanaHeight> {
        height.0.checked_add(1).map(SolanaHeight)
    }

    fn sub_height(height: &SolanaHeight, n: u64) -> Option<SolanaHeight> {
        Some(SolanaHeight(height.0.saturating_sub(n).max(1)))
    }

    fn block_time(&self) -> Duration {
        self.config.block_time
    }

    fn chain_id(&self) -> &Self::ChainId {
        &SolanaChainId
    }

    fn chain_label(&self) -> mercury_core::ChainLabel {
        self.label.clone()
    }
}

impl IbcTypes for SolanaChain {
    type ClientState = SolanaClientState;
    type ConsensusState = SolanaConsensusState;
    type CommitmentProof = SolanaCommitmentProof;
    type Packet = SolanaPacket;
    type PacketCommitment = SolanaPacketCommitment;
    type PacketReceipt = SolanaPacketReceipt;
    type Acknowledgement = SolanaAcknowledgement;

    fn packet_sequence(packet: &SolanaPacket) -> PacketSequence {
        packet.sequence
    }

    fn packet_timeout_timestamp(packet: &SolanaPacket) -> TimeoutTimestamp {
        packet.timeout_timestamp
    }

    fn packet_source_ports(packet: &SolanaPacket) -> Vec<Port> {
        packet
            .payloads
            .iter()
            .map(|p| p.source_port.clone())
            .collect()
    }
}

#[async_trait]
impl ChainStatusQuery for SolanaChain {
    async fn query_chain_status(&self) -> Result<Self::ChainStatus> {
        todo!("query Solana slot and block time via RPC")
    }
}

#[async_trait]
impl ClientQuery<Self> for SolanaChain {
    async fn query_client_state(
        &self,
        _client_id: &Self::ClientId,
        _height: &Self::Height,
    ) -> Result<Self::ClientState> {
        todo!("query IBC client state from Solana program account")
    }

    async fn query_consensus_state(
        &self,
        _client_id: &Self::ClientId,
        _consensus_height: &Self::Height,
        _query_height: &Self::Height,
    ) -> Result<Self::ConsensusState> {
        todo!("query IBC consensus state from Solana program account")
    }

    fn trusting_period(_client_state: &Self::ClientState) -> Option<Duration> {
        todo!("extract trusting period from Solana client state")
    }

    fn client_latest_height(_client_state: &Self::ClientState) -> Self::Height {
        todo!("extract latest height from Solana client state")
    }
}

#[async_trait]
impl PacketStateQuery for SolanaChain {
    async fn query_packet_commitment(
        &self,
        _client_id: &Self::ClientId,
        _sequence: PacketSequence,
        _height: &Self::Height,
    ) -> Result<(Option<SolanaPacketCommitment>, SolanaCommitmentProof)> {
        todo!("query packet commitment from Solana program account with proof")
    }

    async fn query_packet_receipt(
        &self,
        _client_id: &Self::ClientId,
        _sequence: PacketSequence,
        _height: &Self::Height,
    ) -> Result<(Option<SolanaPacketReceipt>, SolanaCommitmentProof)> {
        todo!("query packet receipt from Solana program account with proof")
    }

    async fn query_packet_acknowledgement(
        &self,
        _client_id: &Self::ClientId,
        _sequence: PacketSequence,
        _height: &Self::Height,
    ) -> Result<(Option<SolanaAcknowledgement>, SolanaCommitmentProof)> {
        todo!("query packet acknowledgement from Solana program account with proof")
    }

    async fn query_commitment_sequences(
        &self,
        _client_id: &Self::ClientId,
        _height: &Self::Height,
    ) -> Result<Vec<PacketSequence>> {
        todo!("scan Solana program accounts for outstanding packet commitments")
    }
}

#[async_trait]
impl PacketEvents for SolanaChain {
    type SendPacketEvent = SendPacketEvent;
    type WriteAckEvent = WriteAckEvent;

    fn try_extract_send_packet_event(_event: &SolanaEvent) -> Option<SendPacketEvent> {
        todo!("parse SendPacket from Solana program log event")
    }

    fn try_extract_write_ack_event(_event: &SolanaEvent) -> Option<WriteAckEvent> {
        todo!("parse WriteAck from Solana program log event")
    }

    fn packet_from_send_event(event: &SendPacketEvent) -> &SolanaPacket {
        &event.packet
    }

    fn packet_from_write_ack_event(
        event: &WriteAckEvent,
    ) -> (&SolanaPacket, &SolanaAcknowledgement) {
        (&event.packet, &event.ack)
    }

    async fn query_block_events(&self, _height: &SolanaHeight) -> Result<Vec<SolanaEvent>> {
        todo!("query Solana block for IBC program events at given slot")
    }

    async fn query_send_packet_event(
        &self,
        _client_id: &SolanaClientId,
        _sequence: PacketSequence,
    ) -> Result<Option<SendPacketEvent>> {
        todo!("search Solana transaction history for specific send_packet event")
    }
}

#[async_trait]
impl MessageSender for SolanaChain {
    async fn send_messages(&self, _messages: Vec<SolanaMessage>) -> Result<TxReceipt> {
        todo!("build, sign, and submit Solana transactions")
    }
}

#[async_trait]
impl<C: ChainTypes> ClientPayloadBuilder<C> for SolanaChain {
    type CreateClientPayload = SolanaCreateClientPayload;
    type UpdateClientPayload = SolanaUpdateClientPayload;

    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload> {
        todo!("build Solana light client create payload")
    }

    async fn build_update_client_payload(
        &self,
        _trusted_height: &Self::Height,
        _target_height: &Self::Height,
        _counterparty_client_state: &<C as IbcTypes>::ClientState,
    ) -> Result<Self::UpdateClientPayload>
    where
        C: IbcTypes,
    {
        todo!("build Solana light client update payload with validator proofs")
    }
}

#[async_trait]
impl ClientMessageBuilder<Self> for SolanaChain {
    type CreateClientPayload = SolanaCreateClientPayload;
    type UpdateClientPayload = SolanaUpdateClientPayload;

    async fn build_create_client_message(
        &self,
        _payload: SolanaCreateClientPayload,
    ) -> Result<SolanaMessage> {
        todo!("encode IBC create_client instruction for Solana program")
    }

    async fn build_update_client_message(
        &self,
        _client_id: &SolanaClientId,
        _payload: SolanaUpdateClientPayload,
    ) -> Result<UpdateClientOutput<SolanaMessage>> {
        todo!("encode IBC update_client instruction for Solana program")
    }

    async fn build_register_counterparty_message(
        &self,
        _client_id: &SolanaClientId,
        _counterparty_client_id: &SolanaClientId,
        _counterparty_merkle_prefix: mercury_core::MerklePrefix,
    ) -> Result<SolanaMessage> {
        todo!("encode IBC register_counterparty instruction for Solana program")
    }
}

#[async_trait]
impl PacketMessageBuilder<Self> for SolanaChain {
    async fn build_receive_packet_message(
        &self,
        _packet: &SolanaPacket,
        _proof: SolanaCommitmentProof,
        _proof_height: SolanaHeight,
        _revision: u64,
    ) -> Result<SolanaMessage> {
        todo!("encode IBC recv_packet instruction for Solana program")
    }

    async fn build_ack_packet_message(
        &self,
        _packet: &SolanaPacket,
        _ack: &SolanaAcknowledgement,
        _proof: SolanaCommitmentProof,
        _proof_height: SolanaHeight,
        _revision: u64,
    ) -> Result<SolanaMessage> {
        todo!("encode IBC ack_packet instruction for Solana program")
    }

    async fn build_timeout_packet_message(
        &self,
        _packet: &SolanaPacket,
        _proof: SolanaCommitmentProof,
        _proof_height: SolanaHeight,
        _revision: u64,
    ) -> Result<SolanaMessage> {
        todo!("encode IBC timeout_packet instruction for Solana program")
    }
}

#[derive(Clone, Debug)]
pub struct SolanaMisbehaviourEvidence;

#[async_trait]
impl MisbehaviourDetector<Self> for SolanaChain {
    type UpdateHeader = Vec<u8>;
    type MisbehaviourEvidence = SolanaMisbehaviourEvidence;
    type CounterpartyClientState = SolanaClientState;

    async fn check_for_misbehaviour(
        &self,
        _client_id: &SolanaClientId,
        _update_header: &Self::UpdateHeader,
        _client_state: &Self::CounterpartyClientState,
    ) -> Result<Option<Self::MisbehaviourEvidence>> {
        todo!("check Solana headers for misbehaviour")
    }
}

#[async_trait]
impl MisbehaviourMessageBuilder<Self> for SolanaChain {
    type MisbehaviourEvidence = SolanaMisbehaviourEvidence;

    async fn build_misbehaviour_message(
        &self,
        _client_id: &SolanaClientId,
        _evidence: SolanaMisbehaviourEvidence,
    ) -> Result<SolanaMessage> {
        todo!("build Solana misbehaviour submission message")
    }
}

#[async_trait]
impl MisbehaviourQuery<Self> for SolanaChain {
    type CounterpartyUpdateHeader = Vec<u8>;

    async fn query_consensus_state_heights(
        &self,
        _client_id: &SolanaClientId,
    ) -> Result<Vec<SolanaHeight>> {
        todo!("query consensus state heights from Solana program")
    }

    async fn query_update_client_header(
        &self,
        _client_id: &SolanaClientId,
        _consensus_height: &SolanaHeight,
    ) -> Result<Option<Vec<u8>>> {
        todo!("query update client header from Solana transaction history")
    }
}
