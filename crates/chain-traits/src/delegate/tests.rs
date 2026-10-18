// Every mock method returns a value derived from its inputs that differs from
// the trait default, so a wrapper method that falls back to the default instead
// of forwarding shows up as a mismatch against the inner chain's answer.

use std::marker::PhantomData;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::executor::block_on;
use mercury_core::error::Result;
use mercury_core::{ChainLabel, MembershipProofEntry, MerklePrefix};

use crate::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder, UpdateClientOutput, UpgradeClientPayload,
};
use crate::events::{BlockEventStream, BlockEvents, PacketEvents};
use crate::inner::HasCore;
use crate::queries::{ChainStatusQuery, ClientQuery, MisbehaviourQuery, PacketStateQuery};
use crate::types::{
    ChainTypes, IbcTypes, MessageSender, PacketSequence, Port, TimeoutTimestamp, TxReceipt,
};

pub trait Flavor: Send + Sync + 'static {}

pub struct Plain;
impl Flavor for Plain {}

pub struct Other;
impl Flavor for Other {}

pub struct Mock<T> {
    id: String,
    calls: Mutex<Vec<&'static str>>,
    _flavor: PhantomData<fn() -> T>,
}

impl<T> Mock<T> {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            calls: Mutex::new(Vec::new()),
            _flavor: PhantomData,
        }
    }

    fn hit(&self, name: &'static str) {
        self.calls.lock().unwrap().push(name);
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }
}

pub struct Status {
    height: u64,
    timestamp: u64,
}

pub struct SendEv {
    packet: u64,
}

pub struct AckEv {
    packet: u64,
    ack: u64,
}

impl<T: Flavor> ChainTypes for Mock<T> {
    type Height = u64;
    type Timestamp = u64;
    type ChainId = String;
    type ClientId = String;
    type Event = u64;
    type Message = String;
    type MessageResponse = ();
    type ChainStatus = Status;

    fn chain_status_height(status: &Status) -> &u64 {
        &status.height
    }
    fn chain_status_timestamp(status: &Status) -> &u64 {
        &status.timestamp
    }
    fn chain_status_timestamp_secs(status: &Status) -> u64 {
        status.timestamp / 1000
    }
    fn revision_number(&self) -> u64 {
        self.hit("revision_number");
        11
    }
    fn increment_height(height: &u64) -> Option<u64> {
        height.checked_add(100)
    }
    fn sub_height(height: &u64, n: u64) -> Option<u64> {
        height.checked_sub(n * 2)
    }
    fn block_time(&self) -> Duration {
        self.hit("block_time");
        Duration::from_millis(1234)
    }
    fn max_clock_drift(&self) -> Duration {
        self.hit("max_clock_drift");
        Duration::from_secs(7)
    }
    fn chain_id(&self) -> &String {
        self.hit("chain_id");
        &self.id
    }
    fn chain_label(&self) -> ChainLabel {
        self.hit("chain_label");
        ChainLabel::with_id("mock", self.id.as_str())
    }
}

impl<T: Flavor> IbcTypes for Mock<T> {
    type ClientState = u64;
    type ConsensusState = u64;
    type CommitmentProof = Vec<u8>;
    type Packet = u64;
    type PacketCommitment = u64;
    type PacketReceipt = u64;
    type Acknowledgement = u64;

    fn packet_sequence(packet: &u64) -> PacketSequence {
        PacketSequence(packet + 1)
    }
    fn packet_timeout_timestamp(packet: &u64) -> TimeoutTimestamp {
        TimeoutTimestamp(packet * 10)
    }
    fn packet_source_ports(packet: &u64) -> Vec<Port> {
        vec![Port(format!("port-{packet}"))]
    }
}

#[async_trait]
impl<T: Flavor> MessageSender for Mock<T> {
    async fn send_messages(&self, messages: Vec<String>) -> Result<TxReceipt> {
        self.hit("send_messages");
        Ok(TxReceipt {
            gas_used: Some(messages.len() as u64 * 1000),
            confirmed_at: std::time::Instant::now(),
        })
    }
}

#[async_trait]
impl<T: Flavor> ChainStatusQuery for Mock<T> {
    async fn query_chain_status(&self) -> Result<Status> {
        self.hit("query_chain_status");
        Ok(Status {
            height: 42,
            timestamp: 9000,
        })
    }

    async fn query_latest_height(&self) -> Result<u64> {
        self.hit("query_latest_height");
        Ok(42)
    }
}

fn proof_for(tag: &str, client: &str, seq: PacketSequence, height: u64) -> Vec<u8> {
    format!("{tag}:{client}:{seq}:{height}").into_bytes()
}

#[async_trait]
impl<T: Flavor> PacketStateQuery for Mock<T> {
    async fn query_packet_commitment(
        &self,
        client_id: &String,
        sequence: PacketSequence,
        height: &u64,
    ) -> Result<(Option<u64>, Vec<u8>)> {
        self.hit("query_packet_commitment");
        Ok((
            Some(sequence.0 + 1),
            proof_for("c", client_id, sequence, *height),
        ))
    }

    async fn query_packet_receipt(
        &self,
        client_id: &String,
        sequence: PacketSequence,
        height: &u64,
    ) -> Result<(Option<u64>, Vec<u8>)> {
        self.hit("query_packet_receipt");
        Ok((
            Some(sequence.0 + 2),
            proof_for("r", client_id, sequence, *height),
        ))
    }

    async fn query_packet_acknowledgement(
        &self,
        client_id: &String,
        sequence: PacketSequence,
        height: &u64,
    ) -> Result<(Option<u64>, Vec<u8>)> {
        self.hit("query_packet_acknowledgement");
        Ok((
            Some(sequence.0 + 3),
            proof_for("a", client_id, sequence, *height),
        ))
    }

    async fn query_commitment_sequences(
        &self,
        _client_id: &String,
        height: &u64,
    ) -> Result<Vec<PacketSequence>> {
        self.hit("query_commitment_sequences");
        Ok(vec![PacketSequence(*height), PacketSequence(height + 1)])
    }

    async fn query_ack_sequences(
        &self,
        _client_id: &String,
        height: &u64,
    ) -> Result<Vec<PacketSequence>> {
        self.hit("query_ack_sequences");
        Ok(vec![PacketSequence(height * 2)])
    }

    fn commitment_to_membership_entry(
        &self,
        client_id: &String,
        sequence: PacketSequence,
        commitment: &u64,
        proof: &Vec<u8>,
    ) -> Option<MembershipProofEntry> {
        self.hit("commitment_to_membership_entry");
        Some(MembershipProofEntry {
            path: vec![client_id.clone().into_bytes()],
            value: (sequence.0 + commitment).to_be_bytes().to_vec(),
            proof: proof.clone(),
        })
    }

    fn ack_to_membership_entry(
        &self,
        client_id: &String,
        sequence: PacketSequence,
        ack: &u64,
        proof: &Vec<u8>,
    ) -> Option<MembershipProofEntry> {
        self.hit("ack_to_membership_entry");
        Some(MembershipProofEntry {
            path: vec![client_id.clone().into_bytes(), b"ack".to_vec()],
            value: (sequence.0 * ack).to_be_bytes().to_vec(),
            proof: proof.clone(),
        })
    }
}

#[async_trait]
impl<T: Flavor> PacketEvents for Mock<T> {
    type SendPacketEvent = SendEv;
    type WriteAckEvent = AckEv;

    fn try_extract_send_packet_event(event: &u64) -> Option<SendEv> {
        event.is_multiple_of(2).then_some(SendEv { packet: *event })
    }
    fn try_extract_write_ack_event(event: &u64) -> Option<AckEv> {
        (!event.is_multiple_of(2)).then_some(AckEv {
            packet: *event,
            ack: event + 1,
        })
    }
    fn packet_from_send_event(event: &SendEv) -> &u64 {
        &event.packet
    }
    fn packet_from_write_ack_event(event: &AckEv) -> (&u64, &u64) {
        (&event.packet, &event.ack)
    }
    async fn query_block_events(&self, height: &u64) -> Result<Vec<u64>> {
        self.hit("query_block_events");
        Ok(vec![*height, height + 1])
    }
    async fn query_send_packet_event(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
    ) -> Result<Option<SendEv>> {
        self.hit("query_send_packet_event");
        Ok(Some(SendEv {
            packet: sequence.0 * 3,
        }))
    }
    async fn query_write_ack_event(
        &self,
        _client_id: &String,
        sequence: PacketSequence,
    ) -> Result<Option<AckEv>> {
        self.hit("query_write_ack_event");
        Ok(Some(AckEv {
            packet: sequence.0,
            ack: sequence.0 * 5,
        }))
    }
    async fn subscribe_block_events(&self) -> Result<Option<BlockEventStream<u64, u64>>> {
        self.hit("subscribe_block_events");
        let block = BlockEvents {
            height: 99,
            events: vec![4, 5],
        };
        Ok(Some(futures::stream::iter(vec![Ok(block)]).boxed()))
    }
}

#[async_trait]
impl<T: Flavor> ClientQuery<Self> for Mock<T> {
    async fn query_client_state(&self, client_id: &String, height: &u64) -> Result<u64> {
        self.hit("query_client_state");
        Ok(height + client_id.len() as u64)
    }

    async fn query_consensus_state(
        &self,
        _client_id: &String,
        consensus_height: &u64,
        query_height: &u64,
    ) -> Result<u64> {
        self.hit("query_consensus_state");
        Ok(consensus_height * 1000 + query_height)
    }

    fn trusting_period(client_state: &u64) -> Option<Duration> {
        Some(Duration::from_secs(*client_state))
    }

    fn client_latest_height(client_state: &u64) -> u64 {
        client_state + 3
    }
}

#[async_trait]
impl<T: Flavor, U: Flavor> ClientPayloadBuilder<Mock<U>> for Mock<T> {
    type CreateClientPayload = u64;
    type UpdateClientPayload = Vec<u64>;

    async fn build_create_client_payload(&self) -> Result<u64> {
        self.hit("build_create_client_payload");
        Ok(555)
    }

    async fn build_update_client_payload(
        &self,
        trusted_height: &u64,
        target_height: &u64,
        counterparty_client_state: &<Mock<U> as IbcTypes>::ClientState,
    ) -> Result<Vec<u64>>
    where
        Mock<U>: IbcTypes,
    {
        self.hit("build_update_client_payload");
        // The where-clause keeps the projection opaque, so read it back via Debug.
        let client_state: u64 = format!("{counterparty_client_state:?}").parse()?;
        Ok(vec![*trusted_height, *target_height, client_state])
    }

    fn update_payload_proof_height(&self, payload: &Vec<u64>) -> Option<u64> {
        self.hit("update_payload_proof_height");
        payload.first().map(|h| h + 1)
    }

    fn update_payload_message_height(&self, payload: &Vec<u64>) -> Option<u64> {
        self.hit("update_payload_message_height");
        payload.get(1).map(|h| h + 2)
    }

    fn proof_height_without_update(&self, _src_height: &u64, trusted_height: &u64) -> u64 {
        self.hit("proof_height_without_update");
        *trusted_height
    }

    fn required_dst_timestamp_secs(&self, payload: &Vec<u64>) -> Option<u64> {
        self.hit("required_dst_timestamp_secs");
        Some(payload.len() as u64 + 42)
    }

    async fn build_upgrade_client_payload(&self) -> Result<Option<UpgradeClientPayload>> {
        self.hit("build_upgrade_client_payload");
        Ok(Some(upgrade_payload()))
    }
}

fn upgrade_payload() -> UpgradeClientPayload {
    UpgradeClientPayload {
        plan_height: 77,
        upgraded_client_state: vec![1],
        upgraded_consensus_state: vec![2],
        proof_upgrade_client: vec![3],
        proof_upgrade_consensus_state: vec![4],
    }
}

#[async_trait]
impl<T: Flavor> ClientMessageBuilder<Self> for Mock<T> {
    type CreateClientPayload = u64;
    type UpdateClientPayload = Vec<u64>;

    async fn build_create_client_message(&self, payload: u64) -> Result<String> {
        self.hit("build_create_client_message");
        Ok(format!("create:{payload}"))
    }

    async fn build_update_client_message(
        &self,
        client_id: &String,
        payload: Vec<u64>,
    ) -> Result<UpdateClientOutput<String>> {
        self.hit("build_update_client_message");
        Ok(UpdateClientOutput {
            messages: vec![format!("update:{client_id}:{payload:?}")],
            membership_proof: Some(vec![9]),
        })
    }

    async fn build_register_counterparty_message(
        &self,
        client_id: &String,
        counterparty_client_id: &String,
        counterparty_merkle_prefix: MerklePrefix,
    ) -> Result<String> {
        self.hit("build_register_counterparty_message");
        Ok(format!(
            "register:{client_id}:{counterparty_client_id}:{}",
            counterparty_merkle_prefix.0.len()
        ))
    }

    fn enrich_update_payload(&self, payload: &mut Vec<u64>, proofs: &[MembershipProofEntry]) {
        self.hit("enrich_update_payload");
        payload.push(proofs.len() as u64 + 500);
    }

    fn finalize_batch(
        &self,
        update_output: &mut UpdateClientOutput<String>,
        packet_messages: &mut [String],
    ) {
        self.hit("finalize_batch");
        update_output.membership_proof = None;
        for m in packet_messages {
            m.push_str(":final");
        }
    }

    async fn build_upgrade_client_message(
        &self,
        client_id: &String,
        payload: UpgradeClientPayload,
    ) -> Result<Vec<String>> {
        self.hit("build_upgrade_client_message");
        Ok(vec![format!("upgrade:{client_id}:{}", payload.plan_height)])
    }
}

#[async_trait]
impl<T: Flavor> PacketMessageBuilder<Self> for Mock<T> {
    async fn build_receive_packet_message(
        &self,
        packet: &u64,
        proof: Vec<u8>,
        proof_height: u64,
        revision: u64,
    ) -> Result<String> {
        self.hit("build_receive_packet_message");
        Ok(format!("recv:{packet}:{proof:?}:{proof_height}:{revision}"))
    }

    async fn build_ack_packet_message(
        &self,
        packet: &u64,
        ack: &u64,
        proof: Vec<u8>,
        proof_height: u64,
        revision: u64,
    ) -> Result<String> {
        self.hit("build_ack_packet_message");
        Ok(format!(
            "ack:{packet}:{ack}:{proof:?}:{proof_height}:{revision}"
        ))
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &u64,
        proof: Vec<u8>,
        proof_height: u64,
        revision: u64,
    ) -> Result<String> {
        self.hit("build_timeout_packet_message");
        Ok(format!(
            "timeout:{packet}:{proof:?}:{proof_height}:{revision}"
        ))
    }
}

#[async_trait]
impl<T: Flavor> MisbehaviourDetector<Self> for Mock<T> {
    type UpdateHeader = u64;
    type MisbehaviourEvidence = String;
    type CounterpartyClientState = u64;

    async fn check_for_misbehaviour(
        &self,
        client_id: &String,
        update_header: &u64,
        client_state: &u64,
    ) -> Result<Option<String>> {
        self.hit("check_for_misbehaviour");
        Ok(Some(format!("{client_id}:{update_header}:{client_state}")))
    }
}

#[async_trait]
impl<T: Flavor> MisbehaviourMessageBuilder<Self> for Mock<T> {
    type MisbehaviourEvidence = String;

    async fn build_misbehaviour_message(
        &self,
        client_id: &String,
        evidence: String,
    ) -> Result<String> {
        self.hit("build_misbehaviour_message");
        Ok(format!("misbehaviour:{client_id}:{evidence}"))
    }
}

#[async_trait]
impl<T: Flavor> MisbehaviourQuery<Self> for Mock<T> {
    type CounterpartyUpdateHeader = u64;

    async fn query_consensus_state_heights(&self, client_id: &String) -> Result<Vec<u64>> {
        self.hit("query_consensus_state_heights");
        Ok(vec![client_id.len() as u64, 2, 1])
    }

    async fn query_update_client_header(
        &self,
        _client_id: &String,
        consensus_height: &u64,
    ) -> Result<Option<u64>> {
        self.hit("query_update_client_header");
        Ok(Some(consensus_height * 10))
    }
}

pub struct PlainWrap(Mock<Plain>);

crate::delegate_chain! {
    impl[] PlainWrap => Mock<Plain>
}

pub struct GenericWrap<T: Flavor>(Mock<T>);

crate::delegate_chain! {
    impl[T: Flavor] GenericWrap<T> => Mock<T>
}

pub struct SkipCpbWrap(Mock<Plain>);

crate::delegate_chain! {
    impl[] SkipCpbWrap => Mock<Plain>; skip_cpb
}

// Hand-written impl standing in for the custom cross-chain impl `skip_cpb` exists for.
#[async_trait]
impl ClientPayloadBuilder<Mock<Plain>> for SkipCpbWrap {
    type CreateClientPayload = u64;
    type UpdateClientPayload = Vec<u64>;

    async fn build_create_client_payload(&self) -> Result<u64> {
        Ok(1)
    }

    async fn build_update_client_payload(
        &self,
        _trusted_height: &u64,
        _target_height: &u64,
        _counterparty_client_state: &u64,
    ) -> Result<Vec<u64>> {
        Ok(vec![])
    }
}

fn assert_has_core<W: HasCore<Core = C>, C>() {}

// Runs every forwarded method on `wrapper` and on a fresh inner mock and
// checks both the result and that the wrapper reached its inner mock.
macro_rules! check_forwards {
    ($wrapper:expr, $Wrapper:ty, $Inner:ty) => {{
        let wrapper: $Wrapper = $wrapper;
        let reference: $Inner = Mock::new("chain-a");
        block_on(check_chain_types::<$Wrapper, $Inner>(&wrapper, &reference));
        block_on(check_queries::<$Wrapper, $Inner>(&wrapper, &reference));
        block_on(check_events::<$Wrapper, $Inner>(&wrapper, &reference));
        block_on(check_builders::<$Wrapper, $Inner>(&wrapper, &reference));
        block_on(check_misbehaviour::<$Wrapper, $Inner>(&wrapper, &reference));
        wrapper
    }};
}

async fn check_chain_types<W, I>(wrapper: &W, inner: &I)
where
    W: ChainTypes<Height = u64, Timestamp = u64, ChainId = String, ChainStatus = Status>
        + IbcTypes<Packet = u64>
        + MessageSender<Message = String>,
    I: ChainTypes<Height = u64, Timestamp = u64, ChainId = String, ChainStatus = Status>
        + IbcTypes<Packet = u64>
        + MessageSender<Message = String>,
{
    let status = Status {
        height: 5,
        timestamp: 7000,
    };
    assert_eq!(
        W::chain_status_height(&status),
        I::chain_status_height(&status)
    );
    assert_eq!(
        W::chain_status_timestamp(&status),
        I::chain_status_timestamp(&status)
    );
    assert_eq!(W::chain_status_timestamp_secs(&status), 7);
    assert_eq!(W::increment_height(&5), Some(105));
    assert_eq!(W::increment_height(&u64::MAX), None);
    assert_eq!(W::sub_height(&10, 3), Some(4));
    assert_eq!(W::sub_height(&1, 3), None);

    assert_eq!(wrapper.revision_number(), inner.revision_number());
    assert_eq!(wrapper.block_time(), inner.block_time());
    assert_eq!(wrapper.max_clock_drift(), Duration::from_secs(7));
    assert_eq!(wrapper.chain_id(), inner.chain_id());
    assert_eq!(
        wrapper.chain_label().to_string(),
        inner.chain_label().to_string()
    );

    assert_eq!(W::packet_sequence(&4), PacketSequence(5));
    assert_eq!(W::packet_timeout_timestamp(&4), TimeoutTimestamp(40));
    assert_eq!(W::packet_source_ports(&4), I::packet_source_ports(&4));

    let receipt = wrapper
        .send_messages(vec!["a".into(), "b".into()])
        .await
        .unwrap();
    assert_eq!(receipt.gas_used, Some(2000));
}

async fn check_queries<W, I>(wrapper: &W, inner: &I)
where
    W: ChainStatusQuery<Height = u64, ClientId = String, ChainStatus = Status>
        + PacketStateQuery<
            CommitmentProof = Vec<u8>,
            PacketCommitment = u64,
            Acknowledgement = u64,
            PacketReceipt = u64,
        > + ClientQuery<I, ClientState = u64, ConsensusState = u64>,
    I: ChainStatusQuery<Height = u64, ClientId = String, ChainStatus = Status>
        + PacketStateQuery<
            CommitmentProof = Vec<u8>,
            PacketCommitment = u64,
            Acknowledgement = u64,
            PacketReceipt = u64,
        > + ClientQuery<I, ClientState = u64, ConsensusState = u64>,
{
    let client = "07-tendermint-1".to_string();
    let seq = PacketSequence(3);

    assert_eq!(wrapper.query_chain_status().await.unwrap().height, 42);
    assert_eq!(
        wrapper.query_latest_height().await.unwrap(),
        inner.query_latest_height().await.unwrap()
    );

    let (c, cp) = wrapper
        .query_packet_commitment(&client, seq, &8)
        .await
        .unwrap();
    assert_eq!(
        (c, cp.clone()),
        inner
            .query_packet_commitment(&client, seq, &8)
            .await
            .unwrap()
    );
    let (r, rp) = wrapper
        .query_packet_receipt(&client, seq, &8)
        .await
        .unwrap();
    assert_eq!(
        (r, rp),
        inner.query_packet_receipt(&client, seq, &8).await.unwrap()
    );
    let (a, ap) = wrapper
        .query_packet_acknowledgement(&client, seq, &8)
        .await
        .unwrap();
    assert_eq!(
        (a, ap.clone()),
        inner
            .query_packet_acknowledgement(&client, seq, &8)
            .await
            .unwrap()
    );
    assert_eq!(
        wrapper
            .query_commitment_sequences(&client, &8)
            .await
            .unwrap(),
        vec![PacketSequence(8), PacketSequence(9)]
    );
    assert_eq!(
        wrapper.query_ack_sequences(&client, &8).await.unwrap(),
        vec![PacketSequence(16)]
    );

    let entry = wrapper
        .commitment_to_membership_entry(&client, seq, &c.unwrap(), &cp)
        .expect("forwarded commitment entry, not the None default");
    assert_eq!(entry.value, 7u64.to_be_bytes());
    assert_eq!(entry.proof, cp);
    let entry = wrapper
        .ack_to_membership_entry(&client, seq, &a.unwrap(), &ap)
        .expect("forwarded ack entry, not the None default");
    assert_eq!(entry.value, 18u64.to_be_bytes());
    assert_eq!(entry.path[1], b"ack");

    assert_eq!(wrapper.query_client_state(&client, &8).await.unwrap(), 23);
    assert_eq!(
        wrapper
            .query_consensus_state(&client, &4, &8)
            .await
            .unwrap(),
        4008
    );
    assert_eq!(
        <W as ClientQuery<I>>::trusting_period(&60),
        Some(Duration::from_secs(60))
    );
    assert_eq!(<W as ClientQuery<I>>::client_latest_height(&60), 63);
}

async fn check_events<W, I>(wrapper: &W, _inner: &I)
where
    W: PacketEvents<
            Height = u64,
            Event = u64,
            ClientId = String,
            Packet = u64,
            Acknowledgement = u64,
        >,
    I: PacketEvents<
            Height = u64,
            Event = u64,
            ClientId = String,
            Packet = u64,
            Acknowledgement = u64,
        >,
{
    let send = W::try_extract_send_packet_event(&4).expect("even event is a send");
    assert_eq!(*W::packet_from_send_event(&send), 4);
    assert!(W::try_extract_send_packet_event(&5).is_none());
    let ack = W::try_extract_write_ack_event(&5).expect("odd event is an ack");
    assert_eq!(W::packet_from_write_ack_event(&ack), (&5, &6));
    assert!(W::try_extract_write_ack_event(&4).is_none());

    let client = "c".to_string();
    assert_eq!(wrapper.query_block_events(&10).await.unwrap(), vec![10, 11]);
    let send = wrapper
        .query_send_packet_event(&client, PacketSequence(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*W::packet_from_send_event(&send), 6);
    let ack = wrapper
        .query_write_ack_event(&client, PacketSequence(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(W::packet_from_write_ack_event(&ack), (&2, &10));

    let mut stream = wrapper
        .subscribe_block_events()
        .await
        .unwrap()
        .expect("forwarded stream, not the None default");
    let block = stream.next().await.unwrap().unwrap();
    assert_eq!((block.height, block.events), (99, vec![4, 5]));
}

async fn check_builders<W, I>(wrapper: &W, inner: &I)
where
    W: ClientMessageBuilder<
            I,
            Message = String,
            ClientId = String,
            Packet = u64,
            Acknowledgement = u64,
            CommitmentProof = Vec<u8>,
            CreateClientPayload = u64,
            UpdateClientPayload = Vec<u64>,
        > + PacketMessageBuilder<I>,
    I: ClientMessageBuilder<
            I,
            Message = String,
            ClientId = String,
            CreateClientPayload = u64,
            UpdateClientPayload = Vec<u64>,
        > + IbcTypes<Packet = u64, Acknowledgement = u64, CommitmentProof = Vec<u8>, Height = u64>,
{
    let client = "07-tendermint-1".to_string();
    assert_eq!(
        wrapper.build_create_client_message(5).await.unwrap(),
        "create:5"
    );
    let mut out = wrapper
        .build_update_client_message(&client, vec![1, 2])
        .await
        .unwrap();
    assert_eq!(out.messages, vec!["update:07-tendermint-1:[1, 2]"]);
    assert_eq!(out.membership_proof, Some(vec![9]));
    assert_eq!(
        wrapper
            .build_register_counterparty_message(
                &client,
                &"cp".to_string(),
                MerklePrefix::ibc_default()
            )
            .await
            .unwrap(),
        inner
            .build_register_counterparty_message(
                &client,
                &"cp".to_string(),
                MerklePrefix::ibc_default()
            )
            .await
            .unwrap()
    );

    let mut payload = vec![1];
    wrapper.enrich_update_payload(&mut payload, &[]);
    assert_eq!(
        payload,
        vec![1, 500],
        "enrich hook must reach the inner chain"
    );

    let mut packets = vec!["p".to_string()];
    wrapper.finalize_batch(&mut out, &mut packets);
    assert_eq!(
        packets,
        vec!["p:final"],
        "finalize hook must reach the inner chain"
    );
    assert_eq!(out.membership_proof, None);

    assert_eq!(
        wrapper
            .build_upgrade_client_message(&client, upgrade_payload())
            .await
            .unwrap(),
        vec!["upgrade:07-tendermint-1:77"]
    );

    assert_eq!(
        wrapper
            .build_receive_packet_message(&1, vec![2], 3, 4)
            .await
            .unwrap(),
        "recv:1:[2]:3:4"
    );
    assert_eq!(
        wrapper
            .build_ack_packet_message(&1, &9, vec![2], 3, 4)
            .await
            .unwrap(),
        "ack:1:9:[2]:3:4"
    );
    assert_eq!(
        wrapper
            .build_timeout_packet_message(&1, vec![2], 3, 4)
            .await
            .unwrap(),
        "timeout:1:[2]:3:4"
    );
}

async fn check_misbehaviour<W, I>(wrapper: &W, _inner: &I)
where
    W: MisbehaviourDetector<
            I,
            UpdateHeader = u64,
            CounterpartyClientState = u64,
            MisbehaviourEvidence = String,
        > + MisbehaviourQuery<I, CounterpartyUpdateHeader = u64, ClientId = String>
        + MisbehaviourMessageBuilder<I, Message = String>,
    I: ChainTypes<ClientId = String, Height = u64>,
    <W as MisbehaviourMessageBuilder<I>>::MisbehaviourEvidence: From<String>,
{
    let client = "07-tendermint-1".to_string();
    let evidence = wrapper
        .check_for_misbehaviour(&client, &5, &6)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evidence, "07-tendermint-1:5:6");
    assert_eq!(
        wrapper
            .build_misbehaviour_message(&client, evidence.into())
            .await
            .unwrap(),
        "misbehaviour:07-tendermint-1:07-tendermint-1:5:6"
    );
    assert_eq!(
        wrapper
            .query_consensus_state_heights(&client)
            .await
            .unwrap(),
        vec![15, 2, 1]
    );
    assert_eq!(
        wrapper
            .query_update_client_header(&client, &4)
            .await
            .unwrap(),
        Some(40)
    );
}

async fn check_payload_builder<W, C>(wrapper: &W)
where
    W: ClientPayloadBuilder<
            C,
            Height = u64,
            CreateClientPayload = u64,
            UpdateClientPayload = Vec<u64>,
        >,
    C: IbcTypes<ClientState = u64>,
{
    assert_eq!(wrapper.build_create_client_payload().await.unwrap(), 555);
    let payload = wrapper
        .build_update_client_payload(&10, &20, &30)
        .await
        .unwrap();
    assert_eq!(payload, vec![10, 20, 30]);
    assert_eq!(wrapper.update_payload_proof_height(&payload), Some(11));
    assert_eq!(wrapper.update_payload_message_height(&payload), Some(22));
    assert_eq!(wrapper.required_dst_timestamp_secs(&payload), Some(45));
    assert_eq!(wrapper.proof_height_without_update(&50, &40), 40);
    assert_eq!(
        wrapper
            .build_upgrade_client_payload()
            .await
            .unwrap()
            .map(|p| p.plan_height),
        Some(77)
    );
}

// Inner methods the wrapper must reach; any method missing here fell back to a
// trait default or was answered by the wrapper itself.
const FORWARDED: &[&str] = &[
    "revision_number",
    "block_time",
    "max_clock_drift",
    "chain_id",
    "chain_label",
    "send_messages",
    "query_chain_status",
    "query_latest_height",
    "query_packet_commitment",
    "query_packet_receipt",
    "query_packet_acknowledgement",
    "query_commitment_sequences",
    "query_ack_sequences",
    "commitment_to_membership_entry",
    "ack_to_membership_entry",
    "query_client_state",
    "query_consensus_state",
    "query_block_events",
    "query_send_packet_event",
    "query_write_ack_event",
    "subscribe_block_events",
    "build_create_client_message",
    "build_update_client_message",
    "build_register_counterparty_message",
    "enrich_update_payload",
    "finalize_batch",
    "build_upgrade_client_message",
    "build_receive_packet_message",
    "build_ack_packet_message",
    "build_timeout_packet_message",
    "check_for_misbehaviour",
    "build_misbehaviour_message",
    "query_consensus_state_heights",
    "query_update_client_header",
];

const FORWARDED_CPB: &[&str] = &[
    "build_create_client_payload",
    "build_update_client_payload",
    "update_payload_proof_height",
    "update_payload_message_height",
    "proof_height_without_update",
    "required_dst_timestamp_secs",
    "build_upgrade_client_payload",
];

fn assert_reached(calls: &[&'static str], expected: &[&str]) {
    let missing: Vec<_> = expected.iter().filter(|m| !calls.contains(m)).collect();
    assert!(missing.is_empty(), "never reached inner: {missing:?}");
}

#[test]
fn plain_wrapper_forwards_every_chain_method_to_inner() {
    assert_has_core::<PlainWrap, Mock<Plain>>();
    let wrapper = check_forwards!(PlainWrap(Mock::new("chain-a")), PlainWrap, Mock<Plain>);
    assert_eq!(wrapper.id, "chain-a", "Deref exposes the inner chain");
    assert_reached(&wrapper.0.calls(), FORWARDED);
}

#[test]
fn generic_wrapper_forwards_every_chain_method_to_inner() {
    assert_has_core::<GenericWrap<Other>, Mock<Other>>();
    let wrapper = check_forwards!(
        GenericWrap(Mock::new("chain-a")),
        GenericWrap<Other>,
        Mock<Other>
    );
    assert_reached(&wrapper.0.calls(), FORWARDED);
}

#[test]
fn skip_cpb_wrapper_still_forwards_base_traits() {
    let wrapper = check_forwards!(SkipCpbWrap(Mock::new("chain-a")), SkipCpbWrap, Mock<Plain>);
    assert_reached(&wrapper.0.calls(), FORWARDED);

    // The hand-written payload builder is used, not the inner chain's.
    assert_eq!(
        block_on(
            <SkipCpbWrap as ClientPayloadBuilder<Mock<Plain>>>::build_create_client_payload(
                &wrapper
            )
        )
        .unwrap(),
        1
    );
    assert!(!wrapper.0.calls().contains(&"build_create_client_payload"));
}

#[test]
fn generic_wrapper_forwards_client_payload_builder() {
    let wrapper = GenericWrap::<Other>(Mock::new("chain-a"));
    block_on(check_payload_builder::<_, Mock<Plain>>(&wrapper));
    assert_reached(&wrapper.0.calls(), FORWARDED_CPB);
}

#[test]
fn plain_wrapper_forwards_client_payload_builder() {
    let wrapper = PlainWrap(Mock::new("chain-a"));
    block_on(check_payload_builder::<_, Mock<Other>>(&wrapper));
    assert_reached(&wrapper.0.calls(), FORWARDED_CPB);
}

#[test]
fn skip_cpb_hand_impl_gets_documented_payload_defaults() {
    let wrapper = SkipCpbWrap(Mock::new("chain-a"));
    let cpb = |payload: &Vec<u64>| {
        (
            ClientPayloadBuilder::<Mock<Plain>>::update_payload_proof_height(&wrapper, payload),
            ClientPayloadBuilder::<Mock<Plain>>::update_payload_message_height(&wrapper, payload),
            ClientPayloadBuilder::<Mock<Plain>>::required_dst_timestamp_secs(&wrapper, payload),
        )
    };
    // None means: proof at src_height, message at proof height, submit immediately.
    assert_eq!(cpb(&vec![1, 2, 3]), (None, None, None));
    assert_eq!(
        ClientPayloadBuilder::<Mock<Plain>>::proof_height_without_update(&wrapper, &50, &40),
        50
    );
    assert!(
        block_on(ClientPayloadBuilder::<Mock<Plain>>::build_upgrade_client_payload(&wrapper))
            .unwrap()
            .is_none()
    );
    assert_eq!(wrapper.0.calls(), Vec::<&str>::new());
}

// A chain that relies on every optional trait default.
pub struct Bare;

impl ChainTypes for Bare {
    type Height = u64;
    type Timestamp = u64;
    type ChainId = String;
    type ClientId = String;
    type Event = u64;
    type Message = String;
    type MessageResponse = ();
    type ChainStatus = Status;

    fn chain_status_height(status: &Status) -> &u64 {
        &status.height
    }
    fn chain_status_timestamp(status: &Status) -> &u64 {
        &status.timestamp
    }
    fn chain_status_timestamp_secs(status: &Status) -> u64 {
        status.timestamp
    }
    fn revision_number(&self) -> u64 {
        0
    }
    fn increment_height(height: &u64) -> Option<u64> {
        height.checked_add(1)
    }
    fn sub_height(height: &u64, n: u64) -> Option<u64> {
        height.checked_sub(n)
    }
    fn block_time(&self) -> Duration {
        Duration::from_secs(1)
    }
    fn chain_id(&self) -> &String {
        unreachable!("not used by default-method tests")
    }
    fn chain_label(&self) -> ChainLabel {
        ChainLabel::new("bare")
    }
}

impl IbcTypes for Bare {
    type ClientState = u64;
    type ConsensusState = u64;
    type CommitmentProof = Vec<u8>;
    type Packet = u64;
    type PacketCommitment = u64;
    type PacketReceipt = u64;
    type Acknowledgement = u64;

    fn packet_sequence(packet: &u64) -> PacketSequence {
        PacketSequence(*packet)
    }
    fn packet_timeout_timestamp(packet: &u64) -> TimeoutTimestamp {
        TimeoutTimestamp(*packet)
    }
    fn packet_source_ports(_packet: &u64) -> Vec<Port> {
        vec![]
    }
}

#[async_trait]
impl ChainStatusQuery for Bare {
    async fn query_chain_status(&self) -> Result<Status> {
        Ok(Status {
            height: 31,
            timestamp: 0,
        })
    }
}

#[async_trait]
impl PacketStateQuery for Bare {
    async fn query_packet_commitment(
        &self,
        _client_id: &String,
        _sequence: PacketSequence,
        _height: &u64,
    ) -> Result<(Option<u64>, Vec<u8>)> {
        Ok((None, vec![]))
    }
    async fn query_packet_receipt(
        &self,
        _client_id: &String,
        _sequence: PacketSequence,
        _height: &u64,
    ) -> Result<(Option<u64>, Vec<u8>)> {
        Ok((None, vec![]))
    }
    async fn query_packet_acknowledgement(
        &self,
        _client_id: &String,
        _sequence: PacketSequence,
        _height: &u64,
    ) -> Result<(Option<u64>, Vec<u8>)> {
        Ok((None, vec![]))
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
impl PacketEvents for Bare {
    type SendPacketEvent = SendEv;
    type WriteAckEvent = AckEv;

    fn try_extract_send_packet_event(_event: &u64) -> Option<SendEv> {
        None
    }
    fn try_extract_write_ack_event(_event: &u64) -> Option<AckEv> {
        None
    }
    fn packet_from_send_event(event: &SendEv) -> &u64 {
        &event.packet
    }
    fn packet_from_write_ack_event(event: &AckEv) -> (&u64, &u64) {
        (&event.packet, &event.ack)
    }
    async fn query_block_events(&self, _height: &u64) -> Result<Vec<u64>> {
        Ok(vec![])
    }
    async fn query_send_packet_event(
        &self,
        _client_id: &String,
        _sequence: PacketSequence,
    ) -> Result<Option<SendEv>> {
        Ok(None)
    }
    async fn query_write_ack_event(
        &self,
        _client_id: &String,
        _sequence: PacketSequence,
    ) -> Result<Option<AckEv>> {
        Ok(None)
    }
}

#[async_trait]
impl ClientMessageBuilder<Self> for Bare {
    type CreateClientPayload = u64;
    type UpdateClientPayload = Vec<u64>;

    async fn build_create_client_message(&self, payload: u64) -> Result<String> {
        Ok(payload.to_string())
    }
    async fn build_update_client_message(
        &self,
        _client_id: &String,
        payload: Vec<u64>,
    ) -> Result<UpdateClientOutput<String>> {
        Ok(UpdateClientOutput::messages_only(vec![format!(
            "{payload:?}"
        )]))
    }
    async fn build_register_counterparty_message(
        &self,
        client_id: &String,
        _counterparty_client_id: &String,
        _counterparty_merkle_prefix: MerklePrefix,
    ) -> Result<String> {
        Ok(client_id.clone())
    }
}

#[test]
fn trait_defaults_are_inert() {
    let chain = Bare;
    assert_eq!(chain.max_clock_drift(), Duration::ZERO);
    assert_eq!(block_on(chain.query_latest_height()).unwrap(), 31);
    assert!(block_on(chain.subscribe_block_events()).unwrap().is_none());

    let client = "c".to_string();
    assert!(
        chain
            .commitment_to_membership_entry(&client, PacketSequence(1), &1, &vec![])
            .is_none()
    );
    assert!(
        chain
            .ack_to_membership_entry(&client, PacketSequence(1), &1, &vec![])
            .is_none()
    );

    let mut payload = vec![1, 2];
    chain.enrich_update_payload(&mut payload, &[]);
    assert_eq!(payload, vec![1, 2]);

    let mut out = block_on(chain.build_update_client_message(&client, payload)).unwrap();
    assert_eq!(out.messages, vec!["[1, 2]"]);
    assert!(out.membership_proof.is_none());
    let mut packets = vec!["p".to_string()];
    chain.finalize_batch(&mut out, &mut packets);
    assert_eq!(packets, vec!["p"]);
    assert_eq!(out.messages, vec!["[1, 2]"]);

    assert_eq!(
        block_on(chain.build_upgrade_client_message(&client, upgrade_payload())).unwrap(),
        Vec::<String>::new()
    );
}
