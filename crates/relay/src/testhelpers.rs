use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use mercury_chain_traits::prelude::*;
use mercury_chain_traits::relay::Relay;
use mercury_core::error::{Result, eyre};

#[derive(Clone, Debug)]
pub struct MockMsg;

#[derive(Clone, Debug)]
pub struct MockEvidence;

#[derive(Clone, Debug)]
pub struct MockHeader(pub u64);

#[derive(Clone, Debug)]
pub struct MockClientState;

#[derive(Clone, Debug)]
pub struct MockConsensusState;

#[derive(Clone, Debug)]
pub struct MockPacket;

#[derive(Clone, Debug)]
pub struct MockEvent;

#[derive(Clone, Debug)]
pub struct MockSendPacketEvent;

#[derive(Clone, Debug)]
pub struct MockWriteAckEvent;

#[derive(Clone, Debug)]
pub struct MockCommitmentProof;

#[derive(Debug)]
pub enum HeaderResult {
    Pruned,
    Err,
}

#[derive(Debug)]
pub enum CheckResult {
    Evidence,
    Err,
}

#[derive(Debug)]
pub struct MockState {
    pub latest_height: u64,
    pub consensus_heights: Vec<u64>,
    pub headers: HashMap<u64, HeaderResult>,
    pub check_results: HashMap<u64, CheckResult>,
    pub messages_sent: Vec<MockMsg>,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            latest_height: 100,
            consensus_heights: Vec::new(),
            headers: HashMap::new(),
            check_results: HashMap::new(),
            messages_sent: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct MockChain {
    pub state: Arc<Mutex<MockState>>,
    pub chain_id: String,
}

impl MockChain {
    pub fn new(state: Arc<Mutex<MockState>>) -> Self {
        Self {
            state,
            chain_id: "mock-chain".to_owned(),
        }
    }
}

impl fmt::Debug for MockChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MockChain")
    }
}

impl HasCore for MockChain {
    type Core = Self;
}

impl ChainTypes for MockChain {
    type Height = u64;
    type Timestamp = u64;
    type ChainId = String;
    type ClientId = String;
    type Event = MockEvent;
    type Message = MockMsg;
    type MessageResponse = ();
    type ChainStatus = u64;

    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height {
        status
    }
    fn chain_status_timestamp(_status: &Self::ChainStatus) -> &Self::Timestamp {
        &0
    }
    fn chain_status_timestamp_secs(_status: &Self::ChainStatus) -> u64 {
        0
    }
    fn revision_number(&self) -> u64 {
        0
    }
    fn increment_height(height: &Self::Height) -> Option<Self::Height> {
        height.checked_add(1)
    }
    fn sub_height(height: &Self::Height, n: u64) -> Option<Self::Height> {
        height.checked_sub(n)
    }
    fn block_time(&self) -> Duration {
        Duration::from_secs(1)
    }
    fn chain_id(&self) -> &Self::ChainId {
        &self.chain_id
    }
    fn chain_label(&self) -> mercury_core::ChainLabel {
        mercury_core::ChainLabel::new("mock")
    }
}

impl IbcTypes for MockChain {
    type ClientState = MockClientState;
    type ConsensusState = MockConsensusState;
    type CommitmentProof = MockCommitmentProof;
    type Packet = MockPacket;
    type PacketCommitment = ();
    type PacketReceipt = ();
    type Acknowledgement = ();

    fn packet_sequence(_packet: &Self::Packet) -> PacketSequence {
        PacketSequence(0)
    }
    fn packet_timeout_timestamp(_packet: &Self::Packet) -> TimeoutTimestamp {
        TimeoutTimestamp(0)
    }

    fn packet_source_ports(_packet: &Self::Packet) -> Vec<Port> {
        vec![]
    }
}

#[async_trait]
impl MessageSender for MockChain {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<mercury_chain_traits::types::TxReceipt> {
        let mut state = self.state.lock().unwrap();
        state.messages_sent.extend(messages);
        Ok(mercury_chain_traits::types::TxReceipt {
            gas_used: None,
            confirmed_at: std::time::Instant::now(),
        })
    }
}

#[async_trait]
impl ChainStatusQuery for MockChain {
    async fn query_chain_status(&self) -> Result<Self::ChainStatus> {
        let state = self.state.lock().unwrap();
        Ok(state.latest_height)
    }
}

#[async_trait]
impl ClientQuery<Self> for MockChain {
    async fn query_client_state(
        &self,
        _client_id: &String,
        _height: &u64,
    ) -> Result<MockClientState> {
        Ok(MockClientState)
    }

    async fn query_consensus_state(
        &self,
        _client_id: &String,
        _consensus_height: &u64,
        _query_height: &u64,
    ) -> Result<MockConsensusState> {
        Ok(MockConsensusState)
    }

    fn trusting_period(_client_state: &MockClientState) -> Option<Duration> {
        Some(Duration::from_secs(3600))
    }

    fn client_latest_height(_client_state: &MockClientState) -> u64 {
        100
    }
}

#[async_trait]
impl MisbehaviourDetector<Self> for MockChain {
    type UpdateHeader = MockHeader;
    type MisbehaviourEvidence = MockEvidence;
    type CounterpartyClientState = MockClientState;

    async fn check_for_misbehaviour(
        &self,
        _client_id: &String,
        update_header: &MockHeader,
        _client_state: &MockClientState,
    ) -> Result<Option<MockEvidence>> {
        let state = self.state.lock().unwrap();
        match state.check_results.get(&update_header.0) {
            Some(CheckResult::Evidence) => Ok(Some(MockEvidence)),
            Some(CheckResult::Err) => Err(eyre!("check_for_misbehaviour error")),
            None => Ok(None),
        }
    }
}

#[async_trait]
impl MisbehaviourQuery<Self> for MockChain {
    type CounterpartyUpdateHeader = MockHeader;

    async fn query_consensus_state_heights(&self, _client_id: &String) -> Result<Vec<u64>> {
        let state = self.state.lock().unwrap();
        Ok(state.consensus_heights.clone())
    }

    async fn query_update_client_header(
        &self,
        _client_id: &String,
        consensus_height: &u64,
    ) -> Result<Option<MockHeader>> {
        let state = self.state.lock().unwrap();
        match state.headers.get(consensus_height) {
            Some(HeaderResult::Pruned) => Ok(None),
            Some(HeaderResult::Err) => Err(eyre!("header query error")),
            None => Ok(Some(MockHeader(*consensus_height))),
        }
    }
}

#[async_trait]
impl MisbehaviourMessageBuilder<Self> for MockChain {
    type MisbehaviourEvidence = MockEvidence;

    async fn build_misbehaviour_message(
        &self,
        _client_id: &String,
        _evidence: MockEvidence,
    ) -> Result<MockMsg> {
        Ok(MockMsg)
    }
}

#[async_trait]
impl PacketEvents for MockChain {
    type SendPacketEvent = MockSendPacketEvent;
    type WriteAckEvent = MockWriteAckEvent;

    fn try_extract_send_packet_event(_event: &MockEvent) -> Option<MockSendPacketEvent> {
        None
    }
    fn try_extract_write_ack_event(_event: &MockEvent) -> Option<MockWriteAckEvent> {
        None
    }
    fn packet_from_send_event(_event: &MockSendPacketEvent) -> &MockPacket {
        unimplemented!()
    }
    fn packet_from_write_ack_event(_event: &MockWriteAckEvent) -> (&MockPacket, &()) {
        unimplemented!()
    }
    async fn query_block_events(&self, _height: &u64) -> Result<Vec<MockEvent>> {
        Ok(vec![])
    }
    async fn query_send_packet_event(
        &self,
        _client_id: &String,
        _sequence: PacketSequence,
    ) -> Result<Option<MockSendPacketEvent>> {
        Ok(None)
    }
    async fn query_write_ack_event(
        &self,
        _client_id: &String,
        _sequence: PacketSequence,
    ) -> Result<Option<MockWriteAckEvent>> {
        Ok(None)
    }
}

#[async_trait]
impl ClientPayloadBuilder<Self> for MockChain {
    type CreateClientPayload = ();
    type UpdateClientPayload = ();

    async fn build_create_client_payload(&self) -> Result<()> {
        Ok(())
    }
    async fn build_update_client_payload(
        &self,
        _trusted_height: &u64,
        _target_height: &u64,
        _counterparty_client_state: &MockClientState,
    ) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl ClientMessageBuilder<Self> for MockChain {
    type CreateClientPayload = ();
    type UpdateClientPayload = ();

    async fn build_create_client_message(&self, _payload: ()) -> Result<MockMsg> {
        Ok(MockMsg)
    }
    async fn build_update_client_message(
        &self,
        _client_id: &String,
        _payload: (),
    ) -> Result<UpdateClientOutput<MockMsg>> {
        Ok(UpdateClientOutput::messages_only(vec![]))
    }
    async fn build_register_counterparty_message(
        &self,
        _client_id: &String,
        _counterparty_client_id: &String,
        _counterparty_merkle_prefix: mercury_core::MerklePrefix,
    ) -> Result<MockMsg> {
        Ok(MockMsg)
    }
}

#[async_trait]
impl PacketStateQuery for MockChain {
    async fn query_packet_commitment(
        &self,
        _client_id: &String,
        _sequence: PacketSequence,
        _height: &u64,
    ) -> Result<(Option<()>, MockCommitmentProof)> {
        Ok((None, MockCommitmentProof))
    }
    async fn query_packet_receipt(
        &self,
        _client_id: &String,
        _sequence: PacketSequence,
        _height: &u64,
    ) -> Result<(Option<()>, MockCommitmentProof)> {
        Ok((None, MockCommitmentProof))
    }
    async fn query_packet_acknowledgement(
        &self,
        _client_id: &String,
        _sequence: PacketSequence,
        _height: &u64,
    ) -> Result<(Option<()>, MockCommitmentProof)> {
        Ok((None, MockCommitmentProof))
    }
    async fn query_commitment_sequences(
        &self,
        _client_id: &String,
        _height: &u64,
    ) -> Result<Vec<PacketSequence>> {
        Ok(vec![])
    }
    async fn query_ack_sequences(
        &self,
        _client_id: &String,
        _height: &u64,
    ) -> Result<Vec<PacketSequence>> {
        Ok(vec![])
    }
}

#[async_trait]
impl PacketMessageBuilder<Self> for MockChain {
    async fn build_receive_packet_message(
        &self,
        _packet: &MockPacket,
        _proof: MockCommitmentProof,
        _proof_height: u64,
        _revision: u64,
    ) -> Result<MockMsg> {
        Ok(MockMsg)
    }
    async fn build_ack_packet_message(
        &self,
        _packet: &MockPacket,
        _ack: &(),
        _proof: MockCommitmentProof,
        _proof_height: u64,
        _revision: u64,
    ) -> Result<MockMsg> {
        Ok(MockMsg)
    }
    async fn build_timeout_packet_message(
        &self,
        _packet: &MockPacket,
        _proof: MockCommitmentProof,
        _proof_height: u64,
        _revision: u64,
    ) -> Result<MockMsg> {
        Ok(MockMsg)
    }
}

pub struct MockRelay {
    pub src: MockChain,
    pub dst: MockChain,
    pub src_client_id: String,
    pub dst_client_id: String,
}

impl Relay for MockRelay {
    type SrcChain = MockChain;
    type DstChain = MockChain;

    fn src_chain(&self) -> &MockChain {
        &self.src
    }
    fn dst_chain(&self) -> &MockChain {
        &self.dst
    }
    fn src_client_id(&self) -> &String {
        &self.src_client_id
    }
    fn dst_client_id(&self) -> &String {
        &self.dst_client_id
    }
}
