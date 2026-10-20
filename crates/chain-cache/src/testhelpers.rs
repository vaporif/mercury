//! A probe chain that logs every call and derives return values from its
//! arguments, so tests can assert both delegation and argument passing.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder, UpdateClientOutput, UpgradeClientPayload,
};
use mercury_chain_traits::events::{BlockEventStream, BlockEvents, PacketEvents};
use mercury_chain_traits::inner::HasCore;
use mercury_chain_traits::queries::{
    ChainStatusQuery, ClientQuery, MisbehaviourQuery, PacketStateQuery,
};
use mercury_chain_traits::types::{
    ChainTypes, IbcTypes, MessageSender, PacketSequence, Port, TimeoutTimestamp, TxReceipt,
};
use mercury_core::error::{Result, eyre};
use mercury_core::{ChainLabel, MembershipProofEntry, MerklePrefix};

#[derive(Default, Debug)]
pub struct ProbeState {
    pub calls: Vec<&'static str>,
    pub sent: Vec<Vec<u64>>,
    pub fail_sends: bool,
    pub height: u64,
}

#[derive(Clone, Debug)]
pub struct Probe {
    pub state: Arc<Mutex<ProbeState>>,
    pub chain_id: String,
}

impl Probe {
    pub fn new() -> Self {
        Self {
            state: Arc::default(),
            chain_id: "probe-1".to_owned(),
        }
    }

    pub fn calls(&self) -> Vec<&'static str> {
        self.state.lock().unwrap().calls.clone()
    }

    fn log(&self, call: &'static str) {
        self.state.lock().unwrap().calls.push(call);
    }
}

fn entry(tag: u8, sequence: PacketSequence) -> MembershipProofEntry {
    MembershipProofEntry {
        path: vec![vec![tag]],
        value: sequence.0.to_be_bytes().to_vec(),
        proof: vec![],
    }
}

impl ChainTypes for Probe {
    type Height = u64;
    type Timestamp = u64;
    type ChainId = String;
    type ClientId = String;
    type Event = u64;
    type Message = u64;
    type MessageResponse = ();
    type ChainStatus = (u64, u64);

    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height {
        &status.0
    }
    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp {
        &status.1
    }
    fn chain_status_timestamp_secs(status: &Self::ChainStatus) -> u64 {
        status.1 * 10
    }
    fn revision_number(&self) -> u64 {
        self.log("revision_number");
        7
    }
    fn increment_height(height: &u64) -> Option<u64> {
        height.checked_add(1)
    }
    fn sub_height(height: &u64, n: u64) -> Option<u64> {
        height.checked_sub(n)
    }
    fn block_time(&self) -> Duration {
        Duration::from_secs(2)
    }
    fn max_clock_drift(&self) -> Duration {
        Duration::from_secs(9)
    }
    fn chain_id(&self) -> &String {
        &self.chain_id
    }
    fn chain_label(&self) -> ChainLabel {
        ChainLabel::new("probe")
    }
}

impl IbcTypes for Probe {
    type ClientState = u64;
    type ConsensusState = u64;
    type CommitmentProof = u64;
    type Packet = u64;
    type PacketCommitment = u64;
    type PacketReceipt = u64;
    type Acknowledgement = u64;

    fn packet_sequence(packet: &u64) -> PacketSequence {
        PacketSequence(*packet)
    }
    fn packet_timeout_timestamp(packet: &u64) -> TimeoutTimestamp {
        TimeoutTimestamp(packet * 10)
    }
    fn packet_source_ports(packet: &u64) -> Vec<Port> {
        vec![Port(format!("port-{packet}"))]
    }
}

impl HasCore for Probe {
    type Core = Self;
}

#[async_trait]
impl MessageSender for Probe {
    async fn send_messages(&self, messages: Vec<u64>) -> Result<TxReceipt> {
        let fail = {
            let mut state = self.state.lock().unwrap();
            state.calls.push("send_messages");
            state.sent.push(messages);
            state.fail_sends
        };
        if fail {
            return Err(eyre!("broadcast rejected"));
        }
        Ok(TxReceipt {
            gas_used: Some(21),
            confirmed_at: std::time::Instant::now(),
        })
    }
}

#[async_trait]
impl ChainStatusQuery for Probe {
    async fn query_chain_status(&self) -> Result<(u64, u64)> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("query_chain_status");
        state.height += 1;
        Ok((state.height, 1000))
    }
}

#[async_trait]
impl ClientQuery<Self> for Probe {
    async fn query_client_state(&self, _client_id: &String, height: &u64) -> Result<u64> {
        self.log("query_client_state");
        Ok(height + 1)
    }
    async fn query_consensus_state(
        &self,
        _client_id: &String,
        consensus_height: &u64,
        query_height: &u64,
    ) -> Result<u64> {
        self.log("query_consensus_state");
        Ok(consensus_height + query_height)
    }
    fn trusting_period(client_state: &u64) -> Option<Duration> {
        Some(Duration::from_secs(*client_state))
    }
    fn client_latest_height(client_state: &u64) -> u64 {
        client_state * 2
    }
}

#[async_trait]
impl PacketStateQuery for Probe {
    async fn query_packet_commitment(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
        height: &u64,
    ) -> Result<(Option<u64>, u64)> {
        self.log("query_packet_commitment");
        Ok((Some(sequence.0), *height))
    }
    async fn query_packet_receipt(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
        height: &u64,
    ) -> Result<(Option<u64>, u64)> {
        self.log("query_packet_receipt");
        Ok((Some(sequence.0 + 1), *height))
    }
    async fn query_packet_acknowledgement(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
        height: &u64,
    ) -> Result<(Option<u64>, u64)> {
        self.log("query_packet_acknowledgement");
        Ok((Some(sequence.0 + 2), *height))
    }
    async fn query_commitment_sequences(
        &self,
        _client_id: &String,
        height: &u64,
    ) -> Result<Vec<PacketSequence>> {
        self.log("query_commitment_sequences");
        Ok(vec![PacketSequence(*height)])
    }
    async fn query_ack_sequences(
        &self,
        _client_id: &String,
        height: &u64,
    ) -> Result<Vec<PacketSequence>> {
        self.log("query_ack_sequences");
        Ok(vec![PacketSequence(height + 1)])
    }
    fn commitment_to_membership_entry(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
        _commitment: &u64,
        _proof: &u64,
    ) -> Option<MembershipProofEntry> {
        self.log("commitment_to_membership_entry");
        Some(entry(1, sequence))
    }
    fn ack_to_membership_entry(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
        _ack: &u64,
        _proof: &u64,
    ) -> Option<MembershipProofEntry> {
        self.log("ack_to_membership_entry");
        Some(entry(2, sequence))
    }
}

/// Even events are sends, odd events are write acks.
#[async_trait]
impl PacketEvents for Probe {
    type SendPacketEvent = u64;
    type WriteAckEvent = (u64, u64);

    fn try_extract_send_packet_event(event: &u64) -> Option<u64> {
        event.is_multiple_of(2).then_some(*event)
    }
    fn try_extract_write_ack_event(event: &u64) -> Option<(u64, u64)> {
        (!event.is_multiple_of(2)).then_some((*event, event + 100))
    }
    fn packet_from_send_event(event: &u64) -> &u64 {
        event
    }
    fn packet_from_write_ack_event(event: &(u64, u64)) -> (&u64, &u64) {
        (&event.0, &event.1)
    }
    async fn query_block_events(&self, height: &u64) -> Result<Vec<u64>> {
        self.log("query_block_events");
        Ok(vec![*height, height + 1])
    }
    async fn query_send_packet_event(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
    ) -> Result<Option<u64>> {
        self.log("query_send_packet_event");
        Ok(Some(sequence.0))
    }
    async fn query_write_ack_event(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
    ) -> Result<Option<(u64, u64)>> {
        self.log("query_write_ack_event");
        Ok(Some((sequence.0, 0)))
    }
    async fn subscribe_block_events(&self) -> Result<Option<BlockEventStream<u64, u64>>> {
        self.log("subscribe_block_events");
        let item = Ok(BlockEvents {
            height: 4,
            events: vec![8],
        });
        Ok(Some(Box::pin(futures::stream::iter([item]))))
    }
}

#[async_trait]
impl ClientPayloadBuilder<Self> for Probe {
    type CreateClientPayload = u64;
    type UpdateClientPayload = u64;

    async fn build_create_client_payload(&self) -> Result<u64> {
        self.log("build_create_client_payload");
        Ok(11)
    }
    async fn build_update_client_payload(
        &self,
        trusted_height: &u64,
        target_height: &u64,
        counterparty_client_state: &u64,
    ) -> Result<u64> {
        self.log("build_update_client_payload");
        Ok(trusted_height + target_height + counterparty_client_state)
    }
    fn update_payload_proof_height(&self, payload: &u64) -> Option<u64> {
        self.log("update_payload_proof_height");
        Some(payload + 1)
    }
    fn update_payload_message_height(&self, payload: &u64) -> Option<u64> {
        self.log("update_payload_message_height");
        Some(payload + 2)
    }
    fn proof_height_without_update(&self, _src_height: &u64, trusted_height: &u64) -> u64 {
        self.log("proof_height_without_update");
        *trusted_height
    }
    fn required_dst_timestamp_secs(&self, payload: &u64) -> Option<u64> {
        self.log("required_dst_timestamp_secs");
        Some(payload * 3)
    }
    async fn build_upgrade_client_payload(&self) -> Result<Option<UpgradeClientPayload>> {
        self.log("build_upgrade_client_payload");
        Ok(Some(UpgradeClientPayload {
            plan_height: 42,
            upgraded_client_state: vec![1],
            upgraded_consensus_state: vec![2],
            proof_upgrade_client: vec![3],
            proof_upgrade_consensus_state: vec![4],
        }))
    }
}

#[async_trait]
impl ClientMessageBuilder<Self> for Probe {
    type CreateClientPayload = u64;
    type UpdateClientPayload = u64;

    async fn build_create_client_message(&self, payload: u64) -> Result<u64> {
        self.log("build_create_client_message");
        Ok(payload + 1)
    }
    async fn build_update_client_message(
        &self,
        _client_id: &String,
        payload: u64,
    ) -> Result<UpdateClientOutput<u64>> {
        self.log("build_update_client_message");
        Ok(UpdateClientOutput::messages_only(vec![payload]))
    }
    async fn build_register_counterparty_message(
        &self,
        _client_id: &String,
        counterparty_client_id: &String,
        counterparty_merkle_prefix: MerklePrefix,
    ) -> Result<u64> {
        self.log("build_register_counterparty_message");
        Ok(
            u64::try_from(counterparty_client_id.len() + counterparty_merkle_prefix.0.len())
                .unwrap(),
        )
    }
    fn enrich_update_payload(&self, payload: &mut u64, proofs: &[MembershipProofEntry]) {
        self.log("enrich_update_payload");
        *payload += u64::try_from(proofs.len()).unwrap();
    }
    fn finalize_batch(
        &self,
        update_output: &mut UpdateClientOutput<u64>,
        packet_messages: &mut [u64],
    ) {
        self.log("finalize_batch");
        update_output.membership_proof = Some(vec![u8::try_from(packet_messages.len()).unwrap()]);
        for m in packet_messages {
            *m *= 10;
        }
    }
    async fn build_upgrade_client_message(
        &self,
        _client_id: &String,
        payload: UpgradeClientPayload,
    ) -> Result<Vec<u64>> {
        self.log("build_upgrade_client_message");
        Ok(vec![u64::try_from(payload.plan_height).unwrap()])
    }
}

#[async_trait]
impl MisbehaviourDetector<Self> for Probe {
    type UpdateHeader = u64;
    type MisbehaviourEvidence = u64;
    type CounterpartyClientState = u64;

    async fn check_for_misbehaviour(
        &self,
        _client_id: &String,
        update_header: &u64,
        client_state: &u64,
    ) -> Result<Option<u64>> {
        self.log("check_for_misbehaviour");
        Ok(Some(update_header + client_state))
    }
}

#[async_trait]
impl MisbehaviourMessageBuilder<Self> for Probe {
    type MisbehaviourEvidence = u64;

    async fn build_misbehaviour_message(&self, _client_id: &String, evidence: u64) -> Result<u64> {
        self.log("build_misbehaviour_message");
        Ok(evidence * 2)
    }
}

#[async_trait]
impl MisbehaviourQuery<Self> for Probe {
    type CounterpartyUpdateHeader = u64;

    async fn query_consensus_state_heights(&self, _client_id: &String) -> Result<Vec<u64>> {
        self.log("query_consensus_state_heights");
        Ok(vec![3, 2, 1])
    }
    async fn query_update_client_header(
        &self,
        _client_id: &String,
        consensus_height: &u64,
    ) -> Result<Option<u64>> {
        self.log("query_update_client_header");
        Ok(Some(consensus_height + 50))
    }
}

#[async_trait]
impl PacketMessageBuilder<Self> for Probe {
    async fn build_receive_packet_message(
        &self,
        packet: &u64,
        proof: u64,
        proof_height: u64,
        revision: u64,
    ) -> Result<u64> {
        self.log("build_receive_packet_message");
        Ok(packet + proof + proof_height + revision)
    }
    async fn build_ack_packet_message(
        &self,
        packet: &u64,
        ack: &u64,
        proof: u64,
        proof_height: u64,
        revision: u64,
    ) -> Result<u64> {
        self.log("build_ack_packet_message");
        Ok(packet + ack + proof + proof_height + revision)
    }
    async fn build_timeout_packet_message(
        &self,
        packet: &u64,
        proof: u64,
        proof_height: u64,
        revision: u64,
    ) -> Result<u64> {
        self.log("build_timeout_packet_message");
        Ok(packet * 1000 + proof + proof_height + revision)
    }
}
