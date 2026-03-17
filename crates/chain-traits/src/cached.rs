use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use mercury_core::error::Result;
use tokio::sync::RwLock;

use crate::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder, UpdateClientOutput,
};
use crate::events::PacketEvents;
use crate::inner::HasInner;
use crate::queries::{ChainStatusQuery, ClientQuery, MisbehaviourQuery, PacketStateQuery};
use crate::types::{ChainTypes, IbcTypes, MessageSender, TxReceipt};

struct BoundedCache<V> {
    entries: HashMap<String, V>,
    insert_order: VecDeque<String>,
    cap: usize,
}

impl<V> BoundedCache<V> {
    fn new(cap: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(cap),
            insert_order: VecDeque::with_capacity(cap),
            cap,
        }
    }

    fn get(&self, key: &str) -> Option<&V> {
        self.entries.get(key)
    }

    fn insert(&mut self, key: String, value: V) {
        if let Some(existing) = self.entries.get_mut(&key) {
            *existing = value;
            return;
        }

        if self.entries.len() >= self.cap
            && let Some(oldest) = self.insert_order.pop_front()
        {
            self.entries.remove(&oldest);
        }

        self.insert_order.push_back(key.clone());
        self.entries.insert(key, value);
    }
}

const CLIENT_CACHE_CAP: usize = 20;

type CachedStatus<S> = Arc<RwLock<Option<(S, Instant)>>>;

#[derive(Clone)]
pub struct CachedChain<C: IbcTypes> {
    inner: C,
    status: CachedStatus<C::ChainStatus>,
    client_states: Arc<RwLock<BoundedCache<C::ClientState>>>,
    consensus_states: Arc<RwLock<BoundedCache<C::ConsensusState>>>,
}

impl<C: IbcTypes> CachedChain<C> {
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            status: Arc::new(RwLock::new(None)),
            client_states: Arc::new(RwLock::new(BoundedCache::new(CLIENT_CACHE_CAP))),
            consensus_states: Arc::new(RwLock::new(BoundedCache::new(CLIENT_CACHE_CAP))),
        }
    }
}

// --- ChainTypes passthrough ---

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
}

// --- IbcTypes passthrough ---

impl<C: IbcTypes> IbcTypes for CachedChain<C> {
    type ClientState = C::ClientState;
    type ConsensusState = C::ConsensusState;
    type CommitmentProof = C::CommitmentProof;
    type Packet = C::Packet;
    type PacketCommitment = C::PacketCommitment;
    type PacketReceipt = C::PacketReceipt;
    type Acknowledgement = C::Acknowledgement;

    fn packet_sequence(packet: &Self::Packet) -> u64 {
        C::packet_sequence(packet)
    }

    fn packet_timeout_timestamp(packet: &Self::Packet) -> u64 {
        C::packet_timeout_timestamp(packet)
    }

    fn packet_source_ports(packet: &Self::Packet) -> Vec<String> {
        C::packet_source_ports(packet)
    }
}

// --- HasInner passthrough ---

impl<C: HasInner> HasInner for CachedChain<C> {
    type Inner = C::Inner;
}

// --- MessageSender passthrough ---

#[async_trait]
impl<C: MessageSender + IbcTypes> MessageSender for CachedChain<C> {
    async fn send_messages(&self, messages: Vec<Self::Message>) -> Result<TxReceipt> {
        self.inner.send_messages(messages).await
    }
}

// --- PacketStateQuery passthrough ---

#[async_trait]
impl<C: PacketStateQuery> PacketStateQuery for CachedChain<C> {
    async fn query_packet_commitment(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::PacketCommitment>, Self::CommitmentProof)> {
        self.inner
            .query_packet_commitment(client_id, sequence, height)
            .await
    }

    async fn query_packet_receipt(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::PacketReceipt>, Self::CommitmentProof)> {
        self.inner
            .query_packet_receipt(client_id, sequence, height)
            .await
    }

    async fn query_packet_acknowledgement(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
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
    ) -> Result<Vec<u64>> {
        self.inner
            .query_commitment_sequences(client_id, height)
            .await
    }

    fn commitment_to_membership_entry(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        commitment: &Self::PacketCommitment,
        proof: &Self::CommitmentProof,
    ) -> Option<mercury_core::MembershipProofEntry> {
        self.inner
            .commitment_to_membership_entry(client_id, sequence, commitment, proof)
    }

    fn ack_to_membership_entry(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        ack: &Self::Acknowledgement,
        proof: &Self::CommitmentProof,
    ) -> Option<mercury_core::MembershipProofEntry> {
        self.inner
            .ack_to_membership_entry(client_id, sequence, ack, proof)
    }
}

// --- PacketEvents passthrough ---

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
        sequence: u64,
    ) -> Result<Option<Self::SendPacketEvent>> {
        self.inner
            .query_send_packet_event(client_id, sequence)
            .await
    }
}

// --- ClientPayloadBuilder passthrough ---

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
}

// --- ClientMessageBuilder passthrough ---

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

// --- MisbehaviourDetector passthrough ---

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

// --- MisbehaviourMessageBuilder passthrough ---

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

// --- MisbehaviourQuery passthrough ---

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

// --- PacketMessageBuilder passthrough ---

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

// --- Cache-aware ChainStatusQuery ---

#[async_trait]
impl<C: ChainStatusQuery + IbcTypes> ChainStatusQuery for CachedChain<C>
where
    C::ChainStatus: Clone,
{
    async fn query_chain_status(&self) -> Result<Self::ChainStatus> {
        let ttl = self.inner.block_time() / 2;

        // Fast path: read lock
        {
            let cache = self.status.read().await;
            if let Some((ref status, ts)) = *cache
                && ts.elapsed() < ttl
            {
                return Ok(status.clone());
            }
        }

        // Slow path: write lock with double-check
        let mut cache = self.status.write().await;
        if let Some((ref status, ts)) = *cache
            && ts.elapsed() < ttl
        {
            return Ok(status.clone());
        }

        let status = self.inner.query_chain_status().await?;
        let cloned = status.clone();
        *cache = Some((status, Instant::now()));
        drop(cache);
        Ok(cloned)
    }
}

// --- Cache-aware ClientQuery ---

#[async_trait]
impl<X: ChainTypes, C: ClientQuery<X>> ClientQuery<X> for CachedChain<C> {
    async fn query_client_state(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Self::ClientState> {
        let key = format!("{client_id}:{height}");

        // Fast path: read lock
        {
            let cache = self.client_states.read().await;
            if let Some(state) = cache.get(&key) {
                return Ok(state.clone());
            }
        }

        // Slow path: write lock with double-check
        let mut cache = self.client_states.write().await;
        if let Some(state) = cache.get(&key) {
            return Ok(state.clone());
        }

        let state = self.inner.query_client_state(client_id, height).await?;
        cache.insert(key, state.clone());
        drop(cache);
        Ok(state)
    }

    async fn query_consensus_state(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &X::Height,
        query_height: &Self::Height,
    ) -> Result<Self::ConsensusState> {
        let key = format!("{client_id}:{consensus_height}:{query_height}");

        // Fast path: read lock
        {
            let cache = self.consensus_states.read().await;
            if let Some(state) = cache.get(&key) {
                return Ok(state.clone());
            }
        }

        // Slow path: write lock with double-check
        let mut cache = self.consensus_states.write().await;
        if let Some(state) = cache.get(&key) {
            return Ok(state.clone());
        }

        let state = self
            .inner
            .query_consensus_state(client_id, consensus_height, query_height)
            .await?;
        cache.insert(key, state.clone());
        drop(cache);
        Ok(state)
    }

    fn trusting_period(client_state: &Self::ClientState) -> Option<Duration> {
        C::trusting_period(client_state)
    }

    fn client_latest_height(client_state: &Self::ClientState) -> X::Height {
        C::client_latest_height(client_state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_cache_insert_and_get() {
        let mut cache = BoundedCache::new(3);
        cache.insert("a".into(), 1);
        cache.insert("b".into(), 2);
        assert_eq!(cache.get("a"), Some(&1));
        assert_eq!(cache.get("b"), Some(&2));
        assert_eq!(cache.get("c"), None);
    }

    #[test]
    fn bounded_cache_evicts_oldest() {
        let mut cache = BoundedCache::new(2);
        cache.insert("a".into(), 1);
        cache.insert("b".into(), 2);
        cache.insert("c".into(), 3);
        assert_eq!(cache.get("a"), None); // evicted
        assert_eq!(cache.get("b"), Some(&2));
        assert_eq!(cache.get("c"), Some(&3));
    }

    #[test]
    fn bounded_cache_overwrite_existing() {
        let mut cache = BoundedCache::new(2);
        cache.insert("a".into(), 1);
        cache.insert("a".into(), 10);
        assert_eq!(cache.get("a"), Some(&10));
        // Should not have grown — still at 1 entry
        cache.insert("b".into(), 2);
        cache.insert("c".into(), 3);
        // "a" was inserted first, so it gets evicted
        assert_eq!(cache.get("a"), None);
    }
}
