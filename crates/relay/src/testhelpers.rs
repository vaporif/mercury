use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;

use mercury_chain_traits::builders::UpgradeClientPayload;
use mercury_chain_traits::events::{BlockEventStream, BlockEvents};
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
pub struct MockClientState {
    pub latest_height: u64,
    pub trusting_period: Option<Duration>,
}

#[derive(Clone, Debug)]
pub struct MockConsensusState;

#[derive(Clone, Debug)]
pub struct MockStatus {
    pub height: u64,
    pub timestamp_secs: u64,
}

#[derive(Clone, Debug, Default)]
pub struct MockPacket {
    pub seq: u64,
    pub timeout_secs: u64,
    pub ports: Vec<String>,
}

impl MockPacket {
    pub fn new(seq: u64) -> Self {
        Self {
            seq,
            ..Self::default()
        }
    }

    pub fn on_port(seq: u64, port: &str) -> Self {
        Self {
            seq,
            ports: vec![port.to_owned()],
            ..Self::default()
        }
    }

    pub fn with_timeout(seq: u64, timeout_secs: u64) -> Self {
        Self {
            seq,
            timeout_secs,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug)]
pub enum MockEvent {
    Send(MockPacket),
    Ack(MockPacket),
    Other,
}

#[derive(Clone, Debug)]
pub struct MockSendPacketEvent(pub MockPacket);

#[derive(Clone, Debug)]
pub struct MockWriteAckEvent(pub MockPacket);

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

/// Block yielded by a mock websocket stream: `Ok((height, events))` or a stream error.
pub type MockBlock = std::result::Result<(u64, Vec<MockEvent>), &'static str>;

/// Outcome of one `subscribe_block_events` call.
#[derive(Debug)]
pub enum Subscription {
    Unavailable,
    Err,
    /// Yields `blocks`, then either ends or stays open forever.
    Stream {
        blocks: Vec<MockBlock>,
        keep_open: bool,
    },
}

#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent failure toggles for tests"
)]
pub struct MockState {
    pub latest_height: u64,
    pub timestamp_secs: u64,
    pub max_clock_drift_secs: u64,
    pub status_error: bool,
    pub status_queries: usize,

    pub client_latest_height: u64,
    pub trusting_period: Option<Duration>,

    pub consensus_heights: Vec<u64>,
    pub headers: HashMap<u64, HeaderResult>,
    pub check_results: HashMap<u64, CheckResult>,
    /// Makes `proof_height_without_update` return the trusted height, like an
    /// attested client.
    pub prove_at_trusted: bool,

    pub messages_sent: Vec<MockMsg>,
    /// Message count of each `send_messages` call, in order.
    pub tx_batches: Vec<usize>,
    /// Number of upcoming `send_messages` calls that fail.
    pub send_failures: usize,

    pub commitment_seqs: Vec<u64>,
    pub ack_seqs: Vec<u64>,
    pub sequence_query_error: bool,
    /// Sequences with a receipt on this chain.
    pub received: HashSet<u64>,
    /// Sequences with a commitment on this chain.
    pub committed: HashSet<u64>,
    /// Sequences whose receipt/commitment queries fail.
    pub failing_seqs: HashSet<u64>,

    pub send_packets: HashMap<u64, MockPacket>,
    pub write_acks: HashMap<u64, MockPacket>,
    pub event_lookup_errors: HashSet<u64>,

    pub block_events: HashMap<u64, Vec<MockEvent>>,
    pub block_query_errors: HashSet<u64>,
    pub queried_blocks: Vec<u64>,
    pub subscriptions: VecDeque<Subscription>,
    pub subscribe_calls: usize,

    pub update_payload_error: bool,
    pub update_payload_calls: usize,
    pub required_dst_timestamp: Option<u64>,
    pub update_messages: usize,
    pub update_message_error: bool,
    pub upgrade_plan_height: Option<i64>,
    pub upgrade_messages: usize,

    /// Messages the relay builds per packet (recv/ack read dst state, timeout reads src).
    pub build_msgs: usize,
    pub build_error: bool,
    pub build_calls: usize,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            latest_height: 100,
            timestamp_secs: 0,
            max_clock_drift_secs: 0,
            status_error: false,
            status_queries: 0,
            client_latest_height: 100,
            trusting_period: Some(Duration::from_secs(3600)),
            consensus_heights: Vec::new(),
            headers: HashMap::new(),
            check_results: HashMap::new(),
            prove_at_trusted: false,
            messages_sent: Vec::new(),
            tx_batches: Vec::new(),
            send_failures: 0,
            commitment_seqs: Vec::new(),
            ack_seqs: Vec::new(),
            sequence_query_error: false,
            received: HashSet::new(),
            committed: HashSet::new(),
            failing_seqs: HashSet::new(),
            send_packets: HashMap::new(),
            write_acks: HashMap::new(),
            event_lookup_errors: HashSet::new(),
            block_events: HashMap::new(),
            block_query_errors: HashSet::new(),
            queried_blocks: Vec::new(),
            subscriptions: VecDeque::new(),
            subscribe_calls: 0,
            update_payload_error: false,
            update_payload_calls: 0,
            required_dst_timestamp: None,
            update_messages: 0,
            update_message_error: false,
            upgrade_plan_height: None,
            upgrade_messages: 0,
            build_msgs: 0,
            build_error: false,
            build_calls: 0,
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

    fn lock(&self) -> std::sync::MutexGuard<'_, MockState> {
        self.state.lock().unwrap()
    }

    /// Commitment/receipt lookup: errors for `failing_seqs`, else checks `present`.
    fn packet_state(
        &self,
        sequence: PacketSequence,
        present: fn(&MockState) -> &HashSet<u64>,
    ) -> Result<(Option<()>, MockCommitmentProof)> {
        let (failing, found) = {
            let state = self.lock();
            (
                state.failing_seqs.contains(&sequence.0),
                present(&state).contains(&sequence.0),
            )
        };
        if failing {
            return Err(eyre!("packet state query error"));
        }
        Ok((found.then_some(()), MockCommitmentProof))
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
    type ChainStatus = MockStatus;

    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height {
        &status.height
    }
    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp {
        &status.timestamp_secs
    }
    fn chain_status_timestamp_secs(status: &Self::ChainStatus) -> u64 {
        status.timestamp_secs
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
    fn max_clock_drift(&self) -> Duration {
        Duration::from_secs(self.lock().max_clock_drift_secs)
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

    fn packet_sequence(packet: &Self::Packet) -> PacketSequence {
        PacketSequence(packet.seq)
    }
    fn packet_timeout_timestamp(packet: &Self::Packet) -> TimeoutTimestamp {
        TimeoutTimestamp(packet.timeout_secs)
    }

    fn packet_source_ports(packet: &Self::Packet) -> Vec<Port> {
        packet.ports.iter().cloned().map(Port).collect()
    }
}

#[async_trait]
impl MessageSender for MockChain {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<mercury_chain_traits::types::TxReceipt> {
        let failed = {
            let mut state = self.lock();
            state.tx_batches.push(messages.len());
            if state.send_failures > 0 {
                state.send_failures -= 1;
                true
            } else {
                state.messages_sent.extend(messages);
                false
            }
        };
        if failed {
            return Err(eyre!("send_messages error"));
        }
        Ok(mercury_chain_traits::types::TxReceipt {
            gas_used: Some(1),
            confirmed_at: std::time::Instant::now(),
        })
    }
}

#[async_trait]
impl ChainStatusQuery for MockChain {
    async fn query_chain_status(&self) -> Result<Self::ChainStatus> {
        let mut state = self.lock();
        state.status_queries += 1;
        if state.status_error {
            return Err(eyre!("chain status error"));
        }
        Ok(MockStatus {
            height: state.latest_height,
            timestamp_secs: state.timestamp_secs,
        })
    }
}

#[async_trait]
impl ClientQuery<Self> for MockChain {
    async fn query_client_state(
        &self,
        _client_id: &String,
        _height: &u64,
    ) -> Result<MockClientState> {
        let state = self.lock();
        Ok(MockClientState {
            latest_height: state.client_latest_height,
            trusting_period: state.trusting_period,
        })
    }

    async fn query_consensus_state(
        &self,
        _client_id: &String,
        _consensus_height: &u64,
        _query_height: &u64,
    ) -> Result<MockConsensusState> {
        Ok(MockConsensusState)
    }

    fn trusting_period(client_state: &MockClientState) -> Option<Duration> {
        client_state.trusting_period
    }

    fn client_latest_height(client_state: &MockClientState) -> u64 {
        client_state.latest_height
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
        let state = self.lock();
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
        Ok(self.lock().consensus_heights.clone())
    }

    async fn query_update_client_header(
        &self,
        _client_id: &String,
        consensus_height: &u64,
    ) -> Result<Option<MockHeader>> {
        let state = self.lock();
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

fn into_block_stream(blocks: Vec<MockBlock>, keep_open: bool) -> BlockEventStream<u64, MockEvent> {
    let items = futures::stream::iter(blocks.into_iter().map(|block| {
        block
            .map(|(height, events)| BlockEvents { height, events })
            .map_err(|e| eyre!(e))
    }));
    if keep_open {
        Box::pin(items.chain(futures::stream::pending()))
    } else {
        Box::pin(items)
    }
}

#[async_trait]
impl PacketEvents for MockChain {
    type SendPacketEvent = MockSendPacketEvent;
    type WriteAckEvent = MockWriteAckEvent;

    fn try_extract_send_packet_event(event: &MockEvent) -> Option<MockSendPacketEvent> {
        match event {
            MockEvent::Send(p) => Some(MockSendPacketEvent(p.clone())),
            _ => None,
        }
    }
    fn try_extract_write_ack_event(event: &MockEvent) -> Option<MockWriteAckEvent> {
        match event {
            MockEvent::Ack(p) => Some(MockWriteAckEvent(p.clone())),
            _ => None,
        }
    }
    fn packet_from_send_event(event: &MockSendPacketEvent) -> &MockPacket {
        &event.0
    }
    fn packet_from_write_ack_event(event: &MockWriteAckEvent) -> (&MockPacket, &()) {
        (&event.0, &())
    }
    async fn query_block_events(&self, height: &u64) -> Result<Vec<MockEvent>> {
        let mut state = self.lock();
        state.queried_blocks.push(*height);
        if state.block_query_errors.contains(height) {
            return Err(eyre!("block events query error"));
        }
        Ok(state.block_events.get(height).cloned().unwrap_or_default())
    }
    async fn query_send_packet_event(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
    ) -> Result<Option<MockSendPacketEvent>> {
        let state = self.lock();
        if state.event_lookup_errors.contains(&sequence.0) {
            return Err(eyre!("send packet lookup error"));
        }
        Ok(state
            .send_packets
            .get(&sequence.0)
            .cloned()
            .map(MockSendPacketEvent))
    }
    async fn query_write_ack_event(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
    ) -> Result<Option<MockWriteAckEvent>> {
        let state = self.lock();
        if state.event_lookup_errors.contains(&sequence.0) {
            return Err(eyre!("write ack lookup error"));
        }
        Ok(state
            .write_acks
            .get(&sequence.0)
            .cloned()
            .map(MockWriteAckEvent))
    }
    async fn subscribe_block_events(&self) -> Result<Option<BlockEventStream<u64, MockEvent>>> {
        let mut state = self.lock();
        state.subscribe_calls += 1;
        match state.subscriptions.pop_front() {
            None | Some(Subscription::Unavailable) => Ok(None),
            Some(Subscription::Err) => Err(eyre!("subscribe error")),
            Some(Subscription::Stream { blocks, keep_open }) => {
                Ok(Some(into_block_stream(blocks, keep_open)))
            }
        }
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
        let failed = {
            let mut state = self.lock();
            state.update_payload_calls += 1;
            state.update_payload_error
        };
        if failed {
            return Err(eyre!("update payload error"));
        }
        Ok(())
    }

    fn proof_height_without_update(&self, src_height: &u64, trusted_height: &u64) -> u64 {
        if self.lock().prove_at_trusted {
            *trusted_height
        } else {
            *src_height
        }
    }

    fn required_dst_timestamp_secs(&self, _payload: &()) -> Option<u64> {
        self.lock().required_dst_timestamp
    }

    async fn build_upgrade_client_payload(&self) -> Result<Option<UpgradeClientPayload>> {
        Ok(self
            .lock()
            .upgrade_plan_height
            .map(|plan_height| UpgradeClientPayload {
                plan_height,
                upgraded_client_state: vec![],
                upgraded_consensus_state: vec![],
                proof_upgrade_client: vec![],
                proof_upgrade_consensus_state: vec![],
            }))
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
        let state = self.lock();
        if state.update_message_error {
            return Err(eyre!("update message error"));
        }
        Ok(UpdateClientOutput::messages_only(vec![
            MockMsg;
            state.update_messages
        ]))
    }
    async fn build_register_counterparty_message(
        &self,
        _client_id: &String,
        _counterparty_client_id: &String,
        _counterparty_merkle_prefix: mercury_core::MerklePrefix,
    ) -> Result<MockMsg> {
        Ok(MockMsg)
    }
    async fn build_upgrade_client_message(
        &self,
        _client_id: &String,
        _payload: UpgradeClientPayload,
    ) -> Result<Vec<MockMsg>> {
        Ok(vec![MockMsg; self.lock().upgrade_messages])
    }
}

#[async_trait]
impl PacketStateQuery for MockChain {
    async fn query_packet_commitment(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
        _height: &u64,
    ) -> Result<(Option<()>, MockCommitmentProof)> {
        self.packet_state(sequence, |s| &s.committed)
    }
    async fn query_packet_receipt(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
        _height: &u64,
    ) -> Result<(Option<()>, MockCommitmentProof)> {
        self.packet_state(sequence, |s| &s.received)
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
        let state = self.lock();
        if state.sequence_query_error {
            return Err(eyre!("commitment sequences error"));
        }
        Ok(state
            .commitment_seqs
            .iter()
            .copied()
            .map(PacketSequence)
            .collect())
    }
    async fn query_ack_sequences(
        &self,
        _client_id: &String,
        _height: &u64,
    ) -> Result<Vec<PacketSequence>> {
        let state = self.lock();
        if state.sequence_query_error {
            return Err(eyre!("ack sequences error"));
        }
        Ok(state.ack_seqs.iter().copied().map(PacketSequence).collect())
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

pub type SharedState = Arc<Mutex<MockState>>;

/// Builds a relay over two mock chains, returning handles to both chain states.
pub fn mock_relay(src: MockState, dst: MockState) -> (Arc<MockRelay>, SharedState, SharedState) {
    let src = Arc::new(Mutex::new(src));
    let dst = Arc::new(Mutex::new(dst));
    let relay = MockRelay {
        src: MockChain::new(Arc::clone(&src)),
        dst: MockChain::new(Arc::clone(&dst)),
        src_client_id: "src-client".into(),
        dst_client_id: "dst-client".into(),
    };
    (Arc::new(relay), src, dst)
}

pub fn mock_label() -> mercury_core::ChainLabel {
    mercury_core::ChainLabel::new("mock")
}

fn build_packet_msgs(chain: &MockChain) -> Result<Vec<MockMsg>> {
    let mut state = chain.lock();
    state.build_calls += 1;
    if state.build_error {
        return Err(eyre!("packet build error"));
    }
    Ok(vec![MockMsg; state.build_msgs])
}

#[async_trait]
impl mercury_chain_traits::relay::RelayPacketBuilder for MockRelay {
    async fn build_receive_packet_messages(
        &self,
        _packet: &MockPacket,
        _proof_height: &u64,
        _message_proof_height: Option<&u64>,
    ) -> mercury_chain_traits::relay::PacketBuildResult<MockMsg> {
        Ok((build_packet_msgs(&self.dst)?, vec![]))
    }

    async fn build_ack_packet_messages(
        &self,
        _packet: &MockPacket,
        _ack: &<MockChain as IbcTypes>::Acknowledgement,
        _proof_height: &u64,
        _message_proof_height: Option<&u64>,
    ) -> mercury_chain_traits::relay::PacketBuildResult<MockMsg> {
        Ok((build_packet_msgs(&self.dst)?, vec![]))
    }

    async fn build_timeout_packet_messages(
        &self,
        _packet: &MockPacket,
        _proof_height: &u64,
        _message_proof_height: Option<&u64>,
    ) -> Result<Vec<MockMsg>> {
        build_packet_msgs(&self.src)
    }
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

/// Splits relay events into sorted `(send seqs, ack seqs)`.
pub fn event_seqs(
    events: Vec<mercury_chain_traits::relay::IbcEvent<MockRelay>>,
) -> (Vec<u64>, Vec<u64>) {
    use mercury_chain_traits::relay::IbcEvent;

    let mut sends = Vec::new();
    let mut acks = Vec::new();
    for event in events {
        match event {
            IbcEvent::SendPacket(e) => sends.push(e.0.seq),
            IbcEvent::WriteAck(e) => acks.push(e.0.seq),
        }
    }
    sends.sort_unstable();
    acks.sort_unstable();
    (sends, acks)
}

/// Receives from `rx`, panicking instead of hanging if nothing arrives
/// within an hour of (usually paused) tokio time.
pub async fn recv<T>(rx: &mut tokio::sync::mpsc::Receiver<T>) -> T {
    tokio::time::timeout(Duration::from_secs(3600), rx.recv())
        .await
        .expect("timed out waiting for channel message")
        .expect("channel closed")
}
