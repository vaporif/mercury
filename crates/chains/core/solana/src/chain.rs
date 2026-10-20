use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mercury_attestor::Aggregator;
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
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::signer::keypair::Keypair;

use crate::accounts::{
    self, Ics07Tendermint, Ics26Router, OnChainClientState, OnChainCommitment,
    deserialize_anchor_account, fetch_account, resolve_ics07_program_id,
};
use crate::config::SolanaChainConfig;
use crate::events;
use crate::rpc::SolanaRpcClient;
use crate::tx;
use crate::types::{
    SendPacketEvent, SolanaAcknowledgement, SolanaChainId, SolanaChainStatus, SolanaClientId,
    SolanaClientState, SolanaCommitmentProof, SolanaConsensusState, SolanaCreateClientPayload,
    SolanaEvent, SolanaHeight, SolanaMessage, SolanaPacket, SolanaPacketCommitment,
    SolanaPacketReceipt, SolanaTimestamp, SolanaTxResponse, SolanaUpdateClientPayload,
    WriteAckEvent,
};

/// How long an attested update waits for attestors to finalize the target slot.
/// Solana finalizes ~32 slots (~13s) behind the confirmed tip.
const FINALITY_WAIT: Duration = Duration::from_secs(60);
const FINALITY_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct SolanaChain {
    pub config: SolanaChainConfig,
    pub rpc: SolanaRpcClient,
    pub keypair: Arc<Keypair>,
    pub ics26_program_id: Pubkey,
    pub ics07_program_id: Option<Pubkey>,
    pub alt_cache: Option<solana_message::AddressLookupTableAccount>,
    /// Set when `config.attestation` is configured.
    pub attestor: Option<Arc<Aggregator>>,
    label: mercury_core::ChainLabel,
}

impl SolanaChain {
    pub fn new(config: SolanaChainConfig) -> Result<Self> {
        config.validate()?;
        let rpc = SolanaRpcClient::new(&config);
        let keypair = Arc::new(crate::keys::load_keypair(&config.keypair_path)?);
        let ics26_program_id: Pubkey = config
            .program_id
            .parse()
            .map_err(|e| eyre::eyre!("invalid program_id: {e}"))?;
        let ics07_program_id = config
            .ics07_program_id
            .as_ref()
            .map(|id| id.parse::<Pubkey>())
            .transpose()
            .map_err(|e| eyre::eyre!("invalid ics07_program_id: {e}"))?;
        let attestor = config
            .attestation
            .as_ref()
            .map(|a| {
                Aggregator::new(
                    &a.endpoints,
                    &a.attestor_addresses,
                    a.min_required_sigs as usize,
                    Duration::from_secs(a.timeout_secs),
                )
                .map(Arc::new)
            })
            .transpose()?;
        let label = mercury_core::ChainLabel::new("solana");
        Ok(Self {
            config,
            rpc,
            keypair,
            ics26_program_id,
            ics07_program_id,
            alt_cache: None,
            attestor,
            label,
        })
    }

    /// Load the configured ALT from on-chain and cache it.
    /// Call this after construction before relaying.
    pub async fn load_alt_cache(&mut self) -> Result<()> {
        if let Some(ref alt_addr_str) = self.config.alt_address {
            let alt_pubkey: Pubkey = alt_addr_str
                .parse()
                .map_err(|e| eyre::eyre!("invalid alt_address: {e}"))?;
            let alt_account = crate::alt::lookup_alt(&self.rpc, &alt_pubkey).await?;
            tracing::info!(%alt_pubkey, addresses = alt_account.addresses.len(), "loaded ALT cache");
            self.alt_cache = Some(alt_account);
        }
        Ok(())
    }

    /// Live sequences: a dense scan from 1 (sequential senders), plus every
    /// sequence named by `event_seq` in the router's recent history, since
    /// senders may pick any sequence and nothing on-chain enumerates them.
    async fn scan_sequences(
        &self,
        client_id: &SolanaClientId,
        pda_fn: impl Fn(&str, u64, &Pubkey) -> (Pubkey, u8),
        event_seq: impl Fn(&SolanaEvent) -> Option<u64>,
    ) -> Result<Vec<PacketSequence>> {
        const MAX_CONSECUTIVE_MISSES: u32 = 10;
        let mut checked = std::collections::BTreeSet::new();
        let mut sequences = std::collections::BTreeSet::new();
        let mut consecutive_misses = 0u32;

        for s in 1.. {
            checked.insert(s);
            let (pda, _) = pda_fn(&client_id.0, s, &self.ics26_program_id);
            let account: Option<OnChainCommitment> = fetch_account(&self.rpc, &pda).await?;
            if let Some(commitment) = account {
                // A cleared PDA still marks the sequence as used, so keep scanning past it.
                if !commitment.is_cleared() {
                    sequences.insert(s);
                }
                consecutive_misses = 0;
            } else {
                consecutive_misses += 1;
                if consecutive_misses >= MAX_CONSECUTIVE_MISSES {
                    break;
                }
            }
        }

        for s in self.sequences_from_history(event_seq).await? {
            if !checked.insert(s) {
                continue;
            }
            let (pda, _) = pda_fn(&client_id.0, s, &self.ics26_program_id);
            let account: Option<OnChainCommitment> = fetch_account(&self.rpc, &pda).await?;
            if account.is_some_and(|c| !c.is_cleared()) {
                sequences.insert(s);
            }
        }
        Ok(sequences.into_iter().map(PacketSequence).collect())
    }

    /// Sequences picked out of router events in the most recent
    /// `MAX_HISTORY_SIGNATURES` transactions; older packets are out of reach.
    async fn sequences_from_history(
        &self,
        event_seq: impl Fn(&SolanaEvent) -> Option<u64>,
    ) -> Result<std::collections::BTreeSet<u64>> {
        const PAGE: usize = 1000;
        const MAX_HISTORY_SIGNATURES: usize = 2000;
        let program = self.ics26_program_id.to_string();
        let mut sequences = std::collections::BTreeSet::new();
        let mut before = None;
        let mut seen = 0;

        while seen < MAX_HISTORY_SIGNATURES {
            let page = self
                .rpc
                .get_signatures_for_address(&self.ics26_program_id, before, PAGE)
                .await?;
            let Some(last) = page.last() else { break };
            before = Some(last.signature.parse()?);
            seen += page.len();
            for entry in page.iter().filter(|e| e.err.is_none()) {
                let tx = self.rpc.get_transaction(&entry.signature.parse()?).await?;
                let Some(meta) = tx.transaction.meta else {
                    continue;
                };
                if let solana_transaction_status::option_serializer::OptionSerializer::Some(logs) =
                    meta.log_messages
                {
                    sequences.extend(
                        events::extract_events_from_logs(&logs, &program)
                            .iter()
                            .filter_map(&event_seq),
                    );
                }
            }
            if page.len() < PAGE {
                break;
            }
        }
        Ok(sequences)
    }

    fn commitment_proof(&self) -> SolanaCommitmentProof {
        self.attestor
            .clone()
            .map_or(SolanaCommitmentProof::None, SolanaCommitmentProof::Attested)
    }

    async fn build_attested_create_client_payload(
        aggregator: &Aggregator,
    ) -> Result<SolanaCreateClientPayload> {
        let height = aggregator.latest_height().await?;
        let state = aggregator.state_attestation(height).await?;
        let timestamp_secs = state
            .timestamp
            .ok_or_else(|| eyre::eyre!("state attestation at {height} has no timestamp"))?;
        Ok(SolanaCreateClientPayload::Attested {
            attestor_addresses: aggregator.attestor_addresses().to_vec(),
            min_required_sigs: u32::try_from(aggregator.quorum())?,
            height,
            timestamp_secs,
        })
    }

    /// Updates to the first attested height at or above `target`. Waiting for
    /// finality here means the packets the relayer is about to prove are
    /// already attestable, and a timed-out packet's timeout is already behind
    /// the attested timestamp, so no update goes out only to have its proofs fail.
    async fn build_attested_update_client_payload(
        aggregator: &Aggregator,
        trusted: SolanaHeight,
        target: SolanaHeight,
    ) -> Result<SolanaUpdateClientPayload> {
        let height = aggregator
            .wait_for_height(target.0, FINALITY_WAIT, FINALITY_POLL_INTERVAL)
            .await?;
        eyre::ensure!(
            height > trusted.0,
            "attested height {height} is not above the client's height {trusted}"
        );
        let state = aggregator.state_attestation(height).await?;
        tracing::debug!(
            height,
            signatures = state.signatures.len(),
            "attested Solana state"
        );
        Ok(SolanaUpdateClientPayload::Attested {
            height,
            proof: state.into_proof(),
        })
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
        let slot = self.rpc.get_slot().await?;
        let block_time = self.rpc.get_block_time(slot).await?;
        Ok(SolanaChainStatus {
            height: SolanaHeight(slot),
            timestamp: SolanaTimestamp(block_time.max(0).cast_unsigned()),
        })
    }
}

#[async_trait]
impl ClientQuery<Self> for SolanaChain {
    async fn query_client_state(
        &self,
        client_id: &Self::ClientId,
        _height: &Self::Height,
    ) -> Result<Self::ClientState> {
        let ics07 =
            resolve_ics07_program_id(&self.rpc, &client_id.0, &self.ics26_program_id).await?;
        let (pda, _) = Ics07Tendermint::client_state_pda(&ics07);
        let account = self
            .rpc
            .get_account(&pda)
            .await?
            .ok_or_else(|| eyre::eyre!("client state PDA not found for {client_id}"))?;
        Ok(SolanaClientState(account.data))
    }

    async fn query_consensus_state(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &Self::Height,
        _query_height: &Self::Height,
    ) -> Result<Self::ConsensusState> {
        let ics07 =
            resolve_ics07_program_id(&self.rpc, &client_id.0, &self.ics26_program_id).await?;
        let (pda, _) = Ics07Tendermint::consensus_state_pda(consensus_height.0, &ics07);
        let account =
            self.rpc.get_account(&pda).await?.ok_or_else(|| {
                eyre::eyre!("consensus state not found at height {consensus_height}")
            })?;
        Ok(SolanaConsensusState(account.data))
    }

    fn trusting_period(client_state: &Self::ClientState) -> Option<Duration> {
        let cs: OnChainClientState = deserialize_anchor_account(&client_state.0).ok()?;
        Some(Duration::from_secs(cs.trusting_period))
    }

    #[expect(clippy::expect_used, reason = "trait method is infallible")]
    fn client_latest_height(client_state: &Self::ClientState) -> Self::Height {
        let cs: OnChainClientState =
            deserialize_anchor_account(&client_state.0).expect("invalid client state data");
        SolanaHeight(cs.latest_height.revision_height)
    }
}

#[async_trait]
impl PacketStateQuery for SolanaChain {
    async fn query_packet_commitment(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        _height: &Self::Height,
    ) -> Result<(Option<SolanaPacketCommitment>, SolanaCommitmentProof)> {
        let (pda, _) =
            Ics26Router::packet_commitment_pda(&client_id.0, sequence.0, &self.ics26_program_id);
        let commitment: Option<OnChainCommitment> = fetch_account(&self.rpc, &pda).await?;
        let proof = self.commitment_proof();
        Ok((
            commitment
                .filter(|c| !c.is_cleared())
                .map(|c| SolanaPacketCommitment(c.value.to_vec())),
            proof,
        ))
    }

    async fn query_packet_receipt(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        _height: &Self::Height,
    ) -> Result<(Option<SolanaPacketReceipt>, SolanaCommitmentProof)> {
        let (pda, _) =
            Ics26Router::packet_receipt_pda(&client_id.0, sequence.0, &self.ics26_program_id);
        let exists = self.rpc.get_account(&pda).await?.is_some();
        let proof = self.commitment_proof();
        Ok((exists.then_some(SolanaPacketReceipt), proof))
    }

    async fn query_packet_acknowledgement(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        _height: &Self::Height,
    ) -> Result<(Option<SolanaAcknowledgement>, SolanaCommitmentProof)> {
        let (pda, _) =
            Ics26Router::packet_ack_pda(&client_id.0, sequence.0, &self.ics26_program_id);
        let commitment: Option<OnChainCommitment> = fetch_account(&self.rpc, &pda).await?;
        let proof = self.commitment_proof();
        Ok((
            commitment.map(|c| SolanaAcknowledgement(c.value.to_vec())),
            proof,
        ))
    }

    async fn query_commitment_sequences(
        &self,
        client_id: &Self::ClientId,
        _height: &Self::Height,
    ) -> Result<Vec<PacketSequence>> {
        self.scan_sequences(client_id, Ics26Router::packet_commitment_pda, |event| {
            events::try_decode_send_packet(event)
                .filter(|e| e.packet.source_client_id == client_id.0)
                .map(|e| e.packet.sequence.0)
        })
        .await
    }

    async fn query_ack_sequences(
        &self,
        client_id: &Self::ClientId,
        _height: &Self::Height,
    ) -> Result<Vec<PacketSequence>> {
        self.scan_sequences(client_id, Ics26Router::packet_ack_pda, |event| {
            events::try_decode_write_ack(event)
                .filter(|e| e.packet.dest_client_id == client_id.0)
                .map(|e| e.packet.sequence.0)
        })
        .await
    }
}

#[async_trait]
impl PacketEvents for SolanaChain {
    type SendPacketEvent = SendPacketEvent;
    type WriteAckEvent = WriteAckEvent;

    fn try_extract_send_packet_event(event: &SolanaEvent) -> Option<SendPacketEvent> {
        events::try_decode_send_packet(event)
    }

    fn try_extract_write_ack_event(event: &SolanaEvent) -> Option<WriteAckEvent> {
        events::try_decode_write_ack(event)
    }

    fn packet_from_send_event(event: &SendPacketEvent) -> &SolanaPacket {
        &event.packet
    }

    fn packet_from_write_ack_event(
        event: &WriteAckEvent,
    ) -> (&SolanaPacket, &SolanaAcknowledgement) {
        (&event.packet, &event.ack)
    }

    async fn query_block_events(&self, height: &SolanaHeight) -> Result<Vec<SolanaEvent>> {
        let block = self.rpc.get_block(height.0).await?;
        let program_id_str = self.ics26_program_id.to_string();
        let mut all_events = Vec::new();

        if let Some(txs) = block.transactions {
            for tx in txs {
                if let Some(meta) = tx.meta {
                    if meta.err.is_some() {
                        continue;
                    }
                    if let solana_transaction_status::option_serializer::OptionSerializer::Some(
                        logs,
                    ) = meta.log_messages
                    {
                        let tx_events = events::extract_events_from_logs(&logs, &program_id_str);
                        all_events.extend(tx_events);
                    }
                }
            }
        }
        Ok(all_events)
    }

    async fn query_send_packet_event(
        &self,
        client_id: &SolanaClientId,
        sequence: PacketSequence,
    ) -> Result<Option<SendPacketEvent>> {
        let current_slot = self.rpc.get_slot().await?;
        let start_slot = current_slot.saturating_sub(100);
        for slot in (start_slot..=current_slot).rev() {
            if let Ok(block_events) = self.query_block_events(&SolanaHeight(slot)).await {
                for event in &block_events {
                    if let Some(send_event) = events::try_decode_send_packet(event)
                        && send_event.packet.source_client_id == client_id.0
                        && send_event.packet.sequence == sequence
                    {
                        return Ok(Some(send_event));
                    }
                }
            }
        }
        Ok(None)
    }

    async fn query_write_ack_event(
        &self,
        client_id: &SolanaClientId,
        sequence: PacketSequence,
    ) -> Result<Option<WriteAckEvent>> {
        let current_slot = self.rpc.get_slot().await?;
        let start_slot = current_slot.saturating_sub(100);
        for slot in (start_slot..=current_slot).rev() {
            if let Ok(block_events) = self.query_block_events(&SolanaHeight(slot)).await {
                for event in &block_events {
                    if let Some(ack_event) = events::try_decode_write_ack(event)
                        && ack_event.packet.dest_client_id == client_id.0
                        && ack_event.packet.sequence == sequence
                    {
                        return Ok(Some(ack_event));
                    }
                }
            }
        }
        Ok(None)
    }
}

#[async_trait]
impl MessageSender for SolanaChain {
    async fn send_messages(&self, messages: Vec<SolanaMessage>) -> Result<TxReceipt> {
        let alt_slice = self.alt_cache.as_ref().map(std::slice::from_ref);
        for (i, msg) in messages.iter().enumerate() {
            let tx_groups = split_into_transaction_groups(msg.instructions.clone());
            for (j, group) in tx_groups.into_iter().enumerate() {
                let sig = tx::send_transaction(&self.rpc, &self.keypair, group, alt_slice).await?;
                tracing::info!(%sig, message = i, tx_group = j, "solana transaction confirmed");
            }
        }
        Ok(TxReceipt {
            gas_used: None,
            confirmed_at: std::time::Instant::now(),
        })
    }
}

const SET_COMPUTE_UNIT_LIMIT_DISCRIMINATOR: u8 = 2;

/// A `SolanaMessage` may contain instructions for multiple transactions
/// (e.g. chunk uploads + packet + cleanup). This splits them apart so each
/// group can be sent as an independent transaction.
fn split_into_transaction_groups(
    instructions: Vec<solana_sdk::instruction::Instruction>,
) -> Vec<Vec<solana_sdk::instruction::Instruction>> {
    let compute_budget_program = solana_compute_budget_interface::ID;

    let mut groups = Vec::new();
    let mut current_group = Vec::new();

    for ix in instructions {
        let is_set_cu_limit = ix.program_id == compute_budget_program
            && ix.data.first() == Some(&SET_COMPUTE_UNIT_LIMIT_DISCRIMINATOR);

        if is_set_cu_limit && !current_group.is_empty() {
            groups.push(std::mem::take(&mut current_group));
        }
        current_group.push(ix);
    }

    if !current_group.is_empty() {
        groups.push(current_group);
    }

    groups
}

#[async_trait]
impl<C: ChainTypes> ClientPayloadBuilder<C> for SolanaChain {
    type CreateClientPayload = SolanaCreateClientPayload;
    type UpdateClientPayload = SolanaUpdateClientPayload;

    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload> {
        if let Some(aggregator) = &self.attestor {
            return Self::build_attested_create_client_payload(aggregator).await;
        }
        let slot = self.rpc.get_slot().await?;
        let block_time = self.rpc.get_block_time(slot).await?;
        Ok(SolanaCreateClientPayload::Dummy {
            client_state: slot.to_le_bytes().to_vec(),
            consensus_state: block_time.max(0).cast_unsigned().to_le_bytes().to_vec(),
        })
    }

    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
        _counterparty_client_state: &<C as IbcTypes>::ClientState,
    ) -> Result<Self::UpdateClientPayload>
    where
        C: IbcTypes,
    {
        if let Some(aggregator) = &self.attestor {
            return Self::build_attested_update_client_payload(
                aggregator,
                *trusted_height,
                *target_height,
            )
            .await;
        }
        let slot = self.rpc.get_slot().await?;
        let block_time = self.rpc.get_block_time(slot).await?;
        let mut header = Vec::with_capacity(16);
        header.extend_from_slice(&slot.to_le_bytes());
        header.extend_from_slice(&block_time.max(0).cast_unsigned().to_le_bytes());
        Ok(SolanaUpdateClientPayload::Dummy {
            headers: vec![header],
        })
    }

    fn update_payload_proof_height(
        &self,
        payload: &Self::UpdateClientPayload,
    ) -> Option<Self::Height> {
        match payload {
            SolanaUpdateClientPayload::Attested { height, .. } => Some(SolanaHeight(*height)),
            SolanaUpdateClientPayload::Dummy { .. } => None,
        }
    }

    fn proof_height_without_update(
        &self,
        src_height: &Self::Height,
        trusted_height: &Self::Height,
    ) -> Self::Height {
        attested_proof_height(self.attestor.is_some(), *src_height, *trusted_height)
    }
}

/// An attested client only has consensus states at heights it was updated to,
/// so without an update the proof must use the client's own height.
const fn attested_proof_height(
    attested: bool,
    src_height: SolanaHeight,
    trusted_height: SolanaHeight,
) -> SolanaHeight {
    if attested { trusted_height } else { src_height }
}

#[async_trait]
impl ClientMessageBuilder<Self> for SolanaChain {
    type CreateClientPayload = SolanaCreateClientPayload;
    type UpdateClientPayload = SolanaUpdateClientPayload;

    async fn build_create_client_message(
        &self,
        _payload: SolanaCreateClientPayload,
    ) -> Result<SolanaMessage> {
        eyre::bail!("create_client not yet implemented for Solana-to-Solana")
    }

    async fn build_update_client_message(
        &self,
        _client_id: &SolanaClientId,
        _payload: SolanaUpdateClientPayload,
    ) -> Result<UpdateClientOutput<SolanaMessage>> {
        eyre::bail!("update_client not yet implemented for Solana-to-Solana")
    }

    async fn build_register_counterparty_message(
        &self,
        _client_id: &SolanaClientId,
        _counterparty_client_id: &SolanaClientId,
        _counterparty_merkle_prefix: mercury_core::MerklePrefix,
    ) -> Result<SolanaMessage> {
        eyre::bail!(
            "register_counterparty is not a separate step on Solana — \
             counterparty info is set during add_client"
        )
    }
}

#[async_trait]
impl PacketMessageBuilder<Self> for SolanaChain {
    async fn build_receive_packet_message(
        &self,
        packet: &SolanaPacket,
        _proof: SolanaCommitmentProof,
        proof_height: SolanaHeight,
        _revision: u64,
    ) -> Result<SolanaMessage> {
        let dest_client_id = &packet.dest_client_id;
        let dest_port = &packet
            .payloads
            .first()
            .ok_or_else(|| eyre::eyre!("packet has no payloads"))?
            .dest_port
            .0;
        let sequence = packet.sequence.0;

        let ics07 =
            resolve_ics07_program_id(&self.rpc, dest_client_id, &self.ics26_program_id).await?;
        let (router_pda, _) = Ics26Router::router_state_pda(&self.ics26_program_id);
        let router: accounts::OnChainRouterState = fetch_account(&self.rpc, &router_pda)
            .await?
            .ok_or_else(|| eyre::eyre!("router state PDA not found"))?;
        let app_program =
            accounts::resolve_app_program_id(&self.rpc, dest_port, &self.ics26_program_id).await?;

        let msg_packet = packet.to_msg_packet();

        let msg = crate::ibc_types::MsgRecvPacket {
            packet: msg_packet,
            proof: crate::ibc_types::MsgProof {
                height: proof_height.0,
                data: crate::ibc_types::Delivery::Inline { data: vec![] },
            },
        };

        let params = crate::instructions::PacketParams {
            ics26_program_id: &self.ics26_program_id,
            payer: &self.keypair.pubkey(),
            client_id: dest_client_id,
            port: dest_port,
            sequence,
            ics07_program_id: &ics07,
            consensus_height: proof_height.0,
            access_manager_program_id: &router.am_state.access_manager,
            app_program_id: &app_program,
        };
        let ix = crate::instructions::recv_packet(&params, &msg, vec![])?;

        Ok(SolanaMessage {
            instructions: crate::instructions::with_compute_budget(ix),
        })
    }

    async fn build_ack_packet_message(
        &self,
        packet: &SolanaPacket,
        ack: &SolanaAcknowledgement,
        _proof: SolanaCommitmentProof,
        proof_height: SolanaHeight,
        _revision: u64,
    ) -> Result<SolanaMessage> {
        let source_client_id = &packet.source_client_id;
        let source_port = &packet
            .payloads
            .first()
            .ok_or_else(|| eyre::eyre!("packet has no payloads"))?
            .source_port
            .0;
        let sequence = packet.sequence.0;

        let ics07 =
            resolve_ics07_program_id(&self.rpc, source_client_id, &self.ics26_program_id).await?;
        let (router_pda, _) = Ics26Router::router_state_pda(&self.ics26_program_id);
        let router: accounts::OnChainRouterState = fetch_account(&self.rpc, &router_pda)
            .await?
            .ok_or_else(|| eyre::eyre!("router state PDA not found"))?;
        let app_program =
            accounts::resolve_app_program_id(&self.rpc, source_port, &self.ics26_program_id)
                .await?;

        let msg_packet = packet.to_msg_packet();

        let msg = crate::ibc_types::MsgAckPacket {
            packet: msg_packet,
            acknowledgement: ack.0.clone(),
            proof: crate::ibc_types::MsgProof {
                height: proof_height.0,
                data: crate::ibc_types::Delivery::Inline { data: vec![] },
            },
        };

        let params = crate::instructions::PacketParams {
            ics26_program_id: &self.ics26_program_id,
            payer: &self.keypair.pubkey(),
            client_id: source_client_id,
            port: source_port,
            sequence,
            ics07_program_id: &ics07,
            consensus_height: proof_height.0,
            access_manager_program_id: &router.am_state.access_manager,
            app_program_id: &app_program,
        };
        let ix = crate::instructions::ack_packet(&params, &msg, vec![])?;

        Ok(SolanaMessage {
            instructions: crate::instructions::with_compute_budget(ix),
        })
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &SolanaPacket,
        _proof: SolanaCommitmentProof,
        proof_height: SolanaHeight,
        _revision: u64,
    ) -> Result<SolanaMessage> {
        let source_client_id = &packet.source_client_id;
        let source_port = &packet
            .payloads
            .first()
            .ok_or_else(|| eyre::eyre!("packet has no payloads"))?
            .source_port
            .0;
        let sequence = packet.sequence.0;

        let ics07 =
            resolve_ics07_program_id(&self.rpc, source_client_id, &self.ics26_program_id).await?;
        let (router_pda, _) = Ics26Router::router_state_pda(&self.ics26_program_id);
        let router: accounts::OnChainRouterState = fetch_account(&self.rpc, &router_pda)
            .await?
            .ok_or_else(|| eyre::eyre!("router state PDA not found"))?;
        let app_program =
            accounts::resolve_app_program_id(&self.rpc, source_port, &self.ics26_program_id)
                .await?;

        let msg_packet = packet.to_msg_packet();

        let msg = crate::ibc_types::MsgTimeoutPacket {
            packet: msg_packet,
            proof: crate::ibc_types::MsgProof {
                height: proof_height.0,
                data: crate::ibc_types::Delivery::Inline { data: vec![] },
            },
        };

        let params = crate::instructions::PacketParams {
            ics26_program_id: &self.ics26_program_id,
            payer: &self.keypair.pubkey(),
            client_id: source_client_id,
            port: source_port,
            sequence,
            ics07_program_id: &ics07,
            consensus_height: proof_height.0,
            access_manager_program_id: &router.am_state.access_manager,
            app_program_id: &app_program,
        };
        let ix = crate::instructions::timeout_packet(&params, &msg, vec![])?;

        Ok(SolanaMessage {
            instructions: crate::instructions::with_compute_budget(ix),
        })
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
        tracing::debug!("misbehaviour detection not yet implemented for Solana");
        Ok(None)
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
        eyre::bail!("misbehaviour submission not yet implemented for Solana")
    }
}

#[async_trait]
impl MisbehaviourQuery<Self> for SolanaChain {
    type CounterpartyUpdateHeader = Vec<u8>;

    async fn query_consensus_state_heights(
        &self,
        _client_id: &SolanaClientId,
    ) -> Result<Vec<SolanaHeight>> {
        tracing::debug!("consensus state height scan not yet implemented for Solana");
        Ok(Vec::new())
    }

    async fn query_update_client_header(
        &self,
        _client_id: &SolanaClientId,
        _consensus_height: &SolanaHeight,
    ) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attested_client_proves_at_trusted_height() {
        let proof = attested_proof_height(true, SolanaHeight(90), SolanaHeight(100));
        assert_eq!(proof, SolanaHeight(100));
    }

    #[test]
    fn unattested_client_proves_at_src_height() {
        let proof = attested_proof_height(false, SolanaHeight(90), SolanaHeight(100));
        assert_eq!(proof, SolanaHeight(90));
    }

    use base64::Engine as _;
    use mercury_chain_traits::types::Port;
    use serde_json::{Value, json};
    use solana_compute_budget_interface::ComputeBudgetInstruction;
    use solana_sdk::instruction::{AccountMeta, Instruction};

    use crate::accounts::{AccessManager, IbcApp};
    use crate::rpc::testing::{FakeRpc, anchor_account};
    use crate::types::SolanaPayload;

    const CLIENT: &str = "07-tendermint-0";
    const COUNTERPARTY_CLIENT: &str = "08-wasm-0";

    fn config(keypair_path: std::path::PathBuf) -> SolanaChainConfig {
        SolanaChainConfig {
            rpc_addr: "http://127.0.0.1:8899".into(),
            ws_addr: None,
            program_id: Pubkey::new_unique().to_string(),
            ics07_program_id: None,
            keypair_path,
            block_time: Duration::from_millis(400),
            rpc_timeout_secs: 5,
            rpc_rate_limit: 100,
            alt_address: None,
            skip_pre_verify_threshold: None,
            attestation: None,
        }
    }

    fn chain(rpc: FakeRpc) -> SolanaChain {
        SolanaChain {
            config: config("/unused".into()),
            rpc: rpc.into_client(),
            keypair: Arc::new(Keypair::new()),
            ics26_program_id: Pubkey::new_unique(),
            ics07_program_id: None,
            alt_cache: None,
            attestor: None,
            label: mercury_core::ChainLabel::new("solana"),
        }
    }

    /// Build a chain whose ics26 program id is known up front, so tests can
    /// seed accounts at PDAs derived from it.
    fn chain_with(ics26: Pubkey, rpc: FakeRpc) -> SolanaChain {
        SolanaChain {
            ics26_program_id: ics26,
            ..chain(rpc)
        }
    }

    type WireClientState = (String, u64, u64, u64, u64, u64, (u64, u64), (u64, u64));

    fn client_state_bytes(trusting_period: u64, latest: u64) -> Vec<u8> {
        let body: WireClientState = (
            "cosmoshub-4".into(),
            1,
            3,
            trusting_period,
            2 * trusting_period,
            10,
            (0, 0),
            (4, latest),
        );
        anchor_account("ClientState", &body)
    }

    fn packet(payloads: usize) -> SolanaPacket {
        SolanaPacket {
            source_client_id: CLIENT.into(),
            dest_client_id: COUNTERPARTY_CLIENT.into(),
            sequence: PacketSequence(3),
            timeout_timestamp: TimeoutTimestamp(1_000),
            payloads: (0..payloads)
                .map(|i| SolanaPayload {
                    source_port: Port(format!("src-{i}")),
                    dest_port: Port(format!("dst-{i}")),
                    version: "1".into(),
                    encoding: "json".into(),
                    data: vec![1, 2, 3],
                })
                .collect(),
        }
    }

    fn compute_limit() -> Instruction {
        ComputeBudgetInstruction::set_compute_unit_limit(1)
    }

    fn noop(tag: u8) -> Instruction {
        Instruction::new_with_bytes(Pubkey::new_unique(), &[tag], vec![])
    }

    // --- split_into_transaction_groups ---

    #[test]
    fn split_groups_start_at_each_compute_unit_limit() {
        assert_eq!(
            split_into_transaction_groups(vec![]),
            Vec::<Vec<Instruction>>::new()
        );

        let ixs = vec![
            compute_limit(),
            ComputeBudgetInstruction::set_compute_unit_price(1),
            noop(1),
            compute_limit(),
            ComputeBudgetInstruction::request_heap_frame(256 * 1024),
            noop(2),
        ];
        let groups = split_into_transaction_groups(ixs.clone());
        assert_eq!(groups, vec![ixs[..3].to_vec(), ixs[3..].to_vec()]);
    }

    #[test]
    fn split_keeps_leading_instructions_without_budget_as_own_group() {
        let ixs = vec![noop(1), compute_limit(), noop(2)];
        let groups = split_into_transaction_groups(ixs.clone());
        assert_eq!(groups, vec![vec![ixs[0].clone()], ixs[1..].to_vec()]);
    }

    #[test]
    fn split_ignores_discriminator_two_from_other_programs() {
        // Same first data byte as SetComputeUnitLimit, different program.
        let ixs = vec![compute_limit(), noop(2), noop(2)];
        assert_eq!(split_into_transaction_groups(ixs.clone()), vec![ixs]);
    }

    // --- pure trait helpers ---

    #[test]
    fn height_arithmetic_saturates_and_floors_at_one() {
        assert_eq!(
            SolanaChain::increment_height(&SolanaHeight(7)),
            Some(SolanaHeight(8))
        );
        assert_eq!(SolanaChain::increment_height(&SolanaHeight(u64::MAX)), None);
        assert_eq!(
            SolanaChain::sub_height(&SolanaHeight(10), 3),
            Some(SolanaHeight(7))
        );
        assert_eq!(
            SolanaChain::sub_height(&SolanaHeight(3), 5),
            Some(SolanaHeight(1))
        );
        assert_eq!(
            SolanaChain::sub_height(&SolanaHeight(3), 3),
            Some(SolanaHeight(1))
        );
    }

    #[test]
    fn client_state_helpers_decode_on_chain_layout() {
        let cs = SolanaClientState(client_state_bytes(1_209_600, 4_242));
        assert_eq!(
            <SolanaChain as ClientQuery<SolanaChain>>::trusting_period(&cs),
            Some(Duration::from_hours(336))
        );
        assert_eq!(
            <SolanaChain as ClientQuery<SolanaChain>>::client_latest_height(&cs),
            SolanaHeight(4_242)
        );
        let garbage = SolanaClientState(vec![0; 12]);
        assert_eq!(
            <SolanaChain as ClientQuery<SolanaChain>>::trusting_period(&garbage),
            None
        );
    }

    #[test]
    #[should_panic(expected = "invalid client state data")]
    fn client_latest_height_panics_on_garbage() {
        let _ = <SolanaChain as ClientQuery<SolanaChain>>::client_latest_height(
            &SolanaClientState(vec![1, 2, 3]),
        );
    }

    #[test]
    fn packet_source_ports_lists_every_payload() {
        assert_eq!(
            SolanaChain::packet_source_ports(&packet(2)),
            vec![Port("src-0".into()), Port("src-1".into())]
        );
        assert_eq!(
            SolanaChain::packet_source_ports(&packet(0)),
            Vec::<Port>::new()
        );
    }

    #[test]
    fn proof_height_and_payload_height_without_attestor() {
        let c = chain(FakeRpc::default());
        let proof = <SolanaChain as ClientPayloadBuilder<SolanaChain>>::proof_height_without_update(
            &c,
            &SolanaHeight(90),
            &SolanaHeight(100),
        );
        assert_eq!(proof, SolanaHeight(90));

        let dummy = SolanaUpdateClientPayload::Dummy { headers: vec![] };
        assert_eq!(
            <SolanaChain as ClientPayloadBuilder<SolanaChain>>::update_payload_proof_height(
                &c, &dummy
            ),
            None
        );
        let attested = SolanaUpdateClientPayload::Attested {
            height: 55,
            proof: mercury_attestor::proto::lightclient::AttestationProof::default(),
        };
        assert_eq!(
            <SolanaChain as ClientPayloadBuilder<SolanaChain>>::update_payload_proof_height(
                &c, &attested
            ),
            Some(SolanaHeight(55))
        );
        assert!(matches!(c.commitment_proof(), SolanaCommitmentProof::None));
    }

    // --- construction ---

    fn write_keypair(kp: &Keypair) -> std::path::PathBuf {
        let path =
            // `Pubkey::new_unique` repeats across nextest's per-test processes; the
            // keypair's own random pubkey does not.
            std::env::temp_dir().join(format!("mercury-solana-{}.json", kp.pubkey()));
        std::fs::write(
            &path,
            serde_json::to_string(&kp.to_bytes().to_vec()).unwrap(),
        )
        .unwrap();
        path
    }

    #[test]
    fn new_parses_program_ids_and_loads_keypair() {
        let kp = Keypair::new();
        let path = write_keypair(&kp);
        let ics07 = Pubkey::new_unique();
        let mut cfg = config(path.clone());
        cfg.ics07_program_id = Some(ics07.to_string());
        let program_id: Pubkey = cfg.program_id.parse().unwrap();

        let c = SolanaChain::new(cfg.clone()).unwrap();
        assert_eq!(c.ics26_program_id, program_id);
        assert_eq!(c.ics07_program_id, Some(ics07));
        assert_eq!(c.keypair.pubkey(), kp.pubkey());
        assert!(c.attestor.is_none());

        let mut bad_program = cfg.clone();
        bad_program.program_id = "not-a-pubkey".into();
        let err = SolanaChain::new(bad_program).unwrap_err();
        assert!(err.to_string().contains("invalid program_id"), "{err}");

        let mut bad_ics07 = cfg;
        bad_ics07.ics07_program_id = Some("nope".into());
        let err = SolanaChain::new(bad_ics07).unwrap_err();
        assert!(
            err.to_string().contains("invalid ics07_program_id"),
            "{err}"
        );

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn new_rejects_missing_keypair_file() {
        let err = SolanaChain::new(config("/definitely/missing.json".into())).unwrap_err();
        assert!(
            err.to_string().contains("keypair_path does not exist"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn load_alt_cache_is_noop_without_address_and_errors_when_missing() {
        let mut c = chain(FakeRpc::default());
        c.load_alt_cache().await.unwrap();
        assert!(c.alt_cache.is_none());

        let alt = Pubkey::new_unique();
        c.config.alt_address = Some(alt.to_string());
        let err = c.load_alt_cache().await.unwrap_err();
        assert!(err.to_string().contains("ALT account not found"), "{err}");
        assert!(c.alt_cache.is_none());
    }

    // --- queries ---

    #[tokio::test]
    async fn chain_status_clamps_negative_block_time() {
        let c = chain(FakeRpc {
            slot: Some(50),
            block_time: Some(-5),
            ..FakeRpc::default()
        });
        let status = c.query_chain_status().await.unwrap();
        assert_eq!(status.height, SolanaHeight(50));
        assert_eq!(status.timestamp, SolanaTimestamp(0));
    }

    fn client_account(ics07: Pubkey) -> Vec<u8> {
        anchor_account("Client", &(1u8, CLIENT.to_string(), ics07.to_bytes()))
    }

    #[tokio::test]
    async fn client_and_consensus_state_resolve_through_client_pda() {
        let ics26 = Pubkey::new_unique();
        let ics07 = Pubkey::new_unique();
        let cs_bytes = client_state_bytes(100, 9);
        let rpc = FakeRpc::default()
            .with_account(
                Ics26Router::client_pda(CLIENT, &ics26).0,
                client_account(ics07),
            )
            .with_account(
                Ics07Tendermint::client_state_pda(&ics07).0,
                cs_bytes.clone(),
            )
            .with_account(
                Ics07Tendermint::consensus_state_pda(9, &ics07).0,
                vec![0xC5; 40],
            );
        let c = chain_with(ics26, rpc);
        let id = SolanaClientId(CLIENT.into());

        let cs = c.query_client_state(&id, &SolanaHeight(0)).await.unwrap();
        assert_eq!(cs.0, cs_bytes);
        let cons = c
            .query_consensus_state(&id, &SolanaHeight(9), &SolanaHeight(0))
            .await
            .unwrap();
        assert_eq!(cons.0, vec![0xC5; 40]);

        let err = c
            .query_consensus_state(&id, &SolanaHeight(10), &SolanaHeight(0))
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("consensus state not found at height 10"),
            "{err}"
        );

        let unknown = SolanaClientId("07-tendermint-9".into());
        assert!(
            c.query_client_state(&unknown, &SolanaHeight(0))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn client_state_query_errors_when_ics07_state_missing() {
        let ics26 = Pubkey::new_unique();
        let rpc = FakeRpc::default().with_account(
            Ics26Router::client_pda(CLIENT, &ics26).0,
            client_account(Pubkey::new_unique()),
        );
        let err = chain_with(ics26, rpc)
            .query_client_state(&SolanaClientId(CLIENT.into()), &SolanaHeight(0))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("client state PDA not found"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn packet_state_queries_read_router_pdas() {
        let ics26 = Pubkey::new_unique();
        let rpc = FakeRpc::default()
            .with_account(
                Ics26Router::packet_commitment_pda(CLIENT, 1, &ics26).0,
                anchor_account("Commitment", &[0x11u8; 32]),
            )
            .with_account(
                Ics26Router::packet_receipt_pda(CLIENT, 1, &ics26).0,
                vec![0; 9],
            )
            .with_account(
                Ics26Router::packet_ack_pda(CLIENT, 1, &ics26).0,
                anchor_account("Commitment", &[0x22u8; 32]),
            );
        let c = chain_with(ics26, rpc);
        let id = SolanaClientId(CLIENT.into());
        let h = SolanaHeight(0);

        let (commitment, proof) = c
            .query_packet_commitment(&id, PacketSequence(1), &h)
            .await
            .unwrap();
        assert_eq!(commitment.unwrap().0, vec![0x11; 32]);
        assert!(matches!(proof, SolanaCommitmentProof::None));
        let (receipt, _) = c
            .query_packet_receipt(&id, PacketSequence(1), &h)
            .await
            .unwrap();
        assert!(receipt.is_some());
        let (ack, _) = c
            .query_packet_acknowledgement(&id, PacketSequence(1), &h)
            .await
            .unwrap();
        assert_eq!(ack.unwrap().0, vec![0x22; 32]);

        let (commitment, _) = c
            .query_packet_commitment(&id, PacketSequence(2), &h)
            .await
            .unwrap();
        assert!(commitment.is_none());
        let (receipt, _) = c
            .query_packet_receipt(&id, PacketSequence(2), &h)
            .await
            .unwrap();
        assert!(receipt.is_none());
        let (ack, _) = c
            .query_packet_acknowledgement(&id, PacketSequence(2), &h)
            .await
            .unwrap();
        assert!(ack.is_none());
    }

    fn seed_commitments(ics26: &Pubkey, seqs: &[u64]) -> FakeRpc {
        seqs.iter().fold(FakeRpc::default(), |rpc, &s| {
            rpc.with_account(
                Ics26Router::packet_commitment_pda(CLIENT, s, ics26).0,
                anchor_account("Commitment", &[0xC0u8; 32]),
            )
        })
    }

    /// The router zeroes a commitment on ack/timeout but keeps the PDA.
    fn seed_zeroed_commitments(rpc: FakeRpc, ics26: &Pubkey, seqs: &[u64]) -> FakeRpc {
        seqs.iter().fold(rpc, |rpc, &s| {
            rpc.with_account(
                Ics26Router::packet_commitment_pda(CLIENT, s, ics26).0,
                anchor_account("Commitment", &[0u8; 32]),
            )
        })
    }

    #[tokio::test]
    async fn commitment_scan_skips_zeroed_but_scans_past_them() {
        let ics26 = Pubkey::new_unique();
        let acked: Vec<u64> = (1..=12).collect();
        let rpc = seed_zeroed_commitments(seed_commitments(&ics26, &[13]), &ics26, &acked);
        let c = chain_with(ics26, rpc);
        let seqs = c
            .query_commitment_sequences(&SolanaClientId(CLIENT.into()), &SolanaHeight(0))
            .await
            .unwrap();
        assert_eq!(seqs, vec![PacketSequence(13)]);
    }

    #[tokio::test]
    async fn zeroed_commitment_is_reported_absent() {
        let ics26 = Pubkey::new_unique();
        let c = chain_with(
            ics26,
            seed_zeroed_commitments(FakeRpc::default(), &ics26, &[1]),
        );
        let (commitment, _) = c
            .query_packet_commitment(
                &SolanaClientId(CLIENT.into()),
                PacketSequence(1),
                &SolanaHeight(0),
            )
            .await
            .unwrap();
        assert!(commitment.is_none());
    }

    #[tokio::test]
    async fn commitment_scan_tolerates_gaps_below_ten() {
        let ics26 = Pubkey::new_unique();
        let rpc = seed_commitments(&ics26, &[1, 2, 5, 15]);
        let queries = rpc.account_queries.clone();
        let c = chain_with(ics26, rpc);

        let seqs = c
            .query_commitment_sequences(&SolanaClientId(CLIENT.into()), &SolanaHeight(0))
            .await
            .unwrap();
        assert_eq!(
            seqs,
            vec![
                PacketSequence(1),
                PacketSequence(2),
                PacketSequence(5),
                PacketSequence(15)
            ]
        );
        // Scans 1..=15, then stops after ten consecutive misses (16..=25).
        assert_eq!(queries.lock().unwrap().len(), 25);
    }

    #[tokio::test]
    async fn commitment_scan_stops_after_ten_consecutive_misses() {
        let ics26 = Pubkey::new_unique();
        // 2..=11 missing: the scan gives up before reaching 12.
        let c = chain_with(ics26, seed_commitments(&ics26, &[1, 12]));
        let seqs = c
            .query_commitment_sequences(&SolanaClientId(CLIENT.into()), &SolanaHeight(0))
            .await
            .unwrap();
        assert_eq!(seqs, vec![PacketSequence(1)]);
    }

    // The router takes `MsgSendPacket.sequence` from the caller with no
    // on-chain counter (ics26-router send_packet.rs), so live sequences need
    // not start at 1 or be dense.
    #[tokio::test]
    async fn commitment_scan_finds_caller_chosen_sequences() {
        let ics26 = Pubkey::new_unique();
        let caller_chosen = [50, 10_001, 0x9E37_79B9_7F4A_7C15];
        let mut rpc = seed_commitments(&ics26, &caller_chosen);
        rpc.history = caller_chosen
            .iter()
            .map(|&s| history_tx(&ics26, vec![send_log(s)]))
            .collect();
        let c = chain_with(ics26, rpc);
        let seqs = c
            .query_commitment_sequences(&SolanaClientId(CLIENT.into()), &SolanaHeight(0))
            .await
            .unwrap();
        assert_eq!(seqs, caller_chosen.map(PacketSequence).to_vec());
    }

    #[tokio::test]
    async fn history_scan_keeps_live_sequences_named_for_the_client() {
        let ics26 = Pubkey::new_unique();
        let other = "07-tendermint-9";
        let live = anchor_account("Commitment", &[0xC0u8; 32]);
        let mut rpc = seed_zeroed_commitments(FakeRpc::default(), &ics26, &[600])
            .with_account(
                Ics26Router::packet_commitment_pda(CLIENT, 500, &ics26).0,
                live.clone(),
            )
            .with_account(
                Ics26Router::packet_commitment_pda(CLIENT, 700, &ics26).0,
                live.clone(),
            )
            .with_account(
                Ics26Router::packet_commitment_pda(CLIENT, 800, &ics26).0,
                live.clone(),
            )
            .with_account(
                Ics26Router::packet_commitment_pda(other, 500, &ics26).0,
                live,
            )
            .with_account(
                Ics26Router::packet_ack_pda(COUNTERPARTY_CLIENT, 800, &ics26).0,
                anchor_account("Commitment", &[0xA0u8; 32]),
            );
        let mut failed = tx(&ics26, Some(vec![send_log(700)]), true);
        failed["slot"] = json!(1);
        rpc.history = vec![
            history_tx(&ics26, vec![send_log(500)]),
            // acked or timed out: the router zeroed the commitment
            history_tx(&ics26, vec![send_log(600)]),
            (solana_sdk::signature::Signature::new_unique(), failed),
            history_tx(&ics26, vec![ack_log(800)]),
        ];
        let c = chain_with(ics26, rpc);
        let commitments = async |client: &str| {
            c.query_commitment_sequences(&SolanaClientId(client.into()), &SolanaHeight(0))
                .await
        };
        assert_eq!(
            commitments(CLIENT).await.unwrap(),
            vec![PacketSequence(500)]
        );
        assert_eq!(commitments(other).await.unwrap(), vec![]);
        let acks = c
            .query_ack_sequences(
                &SolanaClientId(COUNTERPARTY_CLIENT.into()),
                &SolanaHeight(0),
            )
            .await
            .unwrap();
        assert_eq!(acks, vec![PacketSequence(800)]);
    }

    #[tokio::test]
    async fn ack_scan_uses_ack_pdas() {
        let ics26 = Pubkey::new_unique();
        let rpc = FakeRpc::default()
            .with_account(
                Ics26Router::packet_ack_pda(CLIENT, 2, &ics26).0,
                anchor_account("Commitment", &[0xA0u8; 32]),
            )
            // A commitment must not be mistaken for an ack.
            .with_account(
                Ics26Router::packet_commitment_pda(CLIENT, 3, &ics26).0,
                anchor_account("Commitment", &[0xC0u8; 32]),
            );
        let seqs = chain_with(ics26, rpc)
            .query_ack_sequences(&SolanaClientId(CLIENT.into()), &SolanaHeight(0))
            .await
            .unwrap();
        assert_eq!(seqs, vec![PacketSequence(2)]);
    }

    #[tokio::test]
    async fn chain_status_propagates_rpc_errors() {
        assert!(
            chain(FakeRpc::default())
                .query_chain_status()
                .await
                .is_err()
        );
    }

    // --- events ---

    type WirePayload = (String, String, String, String, Vec<u8>);
    type WirePacket = (u64, String, String, u64, Vec<WirePayload>);

    fn wire_packet(seq: u64) -> WirePacket {
        (
            seq,
            CLIENT.into(),
            COUNTERPARTY_CLIENT.into(),
            1_000,
            vec![(
                "transfer".into(),
                "transfer".into(),
                "1".into(),
                "json".into(),
                b"hi".to_vec(),
            )],
        )
    }

    fn event_log(name: &str, body: &impl borsh::BorshSerialize) -> String {
        let mut data = accounts::anchor_discriminator("event", name).to_vec();
        data.extend(borsh::to_vec(body).unwrap());
        format!(
            "Program data: {}",
            base64::engine::general_purpose::STANDARD.encode(data)
        )
    }

    fn send_log(seq: u64) -> String {
        event_log(
            "SendPacketEvent",
            &(CLIENT.to_string(), seq, wire_packet(seq), 1_000u64),
        )
    }

    fn ack_log(seq: u64) -> String {
        event_log(
            "WriteAcknowledgementEvent",
            &(
                COUNTERPARTY_CLIENT.to_string(),
                seq,
                wire_packet(seq),
                vec![b"ok".to_vec()],
            ),
        )
    }

    fn tx(program: &Pubkey, logs: Option<Vec<String>>, failed: bool) -> Value {
        let mut meta = json!({
            "err": null,
            "status": { "Ok": null },
            "fee": 5000,
            "preBalances": [],
            "postBalances": [],
        });
        if failed {
            meta["err"] = json!("AccountInUse");
            meta["status"] = json!({ "Err": "AccountInUse" });
        }
        if let Some(lines) = logs {
            let mut wrapped = vec![format!("Program {program} invoke [1]")];
            wrapped.extend(lines);
            wrapped.push(format!("Program {program} success"));
            meta["logMessages"] = json!(wrapped);
        }
        json!({ "transaction": ["", "base64"], "meta": meta })
    }

    fn history_tx(
        program: &Pubkey,
        logs: Vec<String>,
    ) -> (solana_sdk::signature::Signature, Value) {
        let mut entry = tx(program, Some(logs), false);
        entry["slot"] = json!(1);
        (solana_sdk::signature::Signature::new_unique(), entry)
    }

    fn block(txs: &[Value]) -> Value {
        json!({
            "previousBlockhash": "11111111111111111111111111111111",
            "blockhash": "11111111111111111111111111111111",
            "parentSlot": 0,
            "transactions": txs,
            "blockTime": null,
            "blockHeight": null,
        })
    }

    #[tokio::test]
    async fn block_events_skip_failed_transactions() {
        let ics26 = Pubkey::new_unique();
        let mut rpc = FakeRpc::default();
        rpc.blocks.insert(
            10,
            block(&[
                tx(&ics26, Some(vec![send_log(1)]), false),
                tx(&ics26, Some(vec![send_log(2)]), true),
                tx(&ics26, None, false),
                // Emitted by a different program: not ours.
                tx(&Pubkey::new_unique(), Some(vec![send_log(3)]), false),
            ]),
        );
        let c = chain_with(ics26, rpc);

        let events = c.query_block_events(&SolanaHeight(10)).await.unwrap();
        assert_eq!(events.len(), 1);
        let decoded = SolanaChain::try_extract_send_packet_event(&events[0]).unwrap();
        assert_eq!(decoded.packet.sequence, PacketSequence(1));
        assert_eq!(events[0].program_id, ics26.to_string());

        assert!(c.query_block_events(&SolanaHeight(11)).await.is_err());
    }

    #[tokio::test]
    async fn send_packet_lookup_scans_recent_blocks() {
        let ics26 = Pubkey::new_unique();
        let mut rpc = FakeRpc {
            slot: Some(200),
            ..FakeRpc::default()
        };
        rpc.blocks.insert(
            150,
            block(&[tx(&ics26, Some(vec![send_log(4), ack_log(4)]), false)]),
        );
        // Outside the 100-slot lookback window.
        rpc.blocks
            .insert(99, block(&[tx(&ics26, Some(vec![send_log(5)]), false)]));
        let c = chain_with(ics26, rpc);
        let client = SolanaClientId(CLIENT.into());

        let found = c
            .query_send_packet_event(&client, PacketSequence(4))
            .await
            .unwrap();
        assert_eq!(found.unwrap().packet.sequence, PacketSequence(4));
        assert!(
            c.query_send_packet_event(&client, PacketSequence(5))
                .await
                .unwrap()
                .is_none()
        );
        let other = SolanaClientId("other".into());
        assert!(
            c.query_send_packet_event(&other, PacketSequence(4))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn write_ack_lookup_matches_dest_client() {
        let ics26 = Pubkey::new_unique();
        let mut rpc = FakeRpc {
            slot: Some(5),
            ..FakeRpc::default()
        };
        rpc.blocks.insert(
            3,
            block(&[tx(&ics26, Some(vec![send_log(4), ack_log(4)]), false)]),
        );
        let c = chain_with(ics26, rpc);

        let found = c
            .query_write_ack_event(
                &SolanaClientId(COUNTERPARTY_CLIENT.into()),
                PacketSequence(4),
            )
            .await
            .unwrap()
            .unwrap();
        let (packet, ack) = SolanaChain::packet_from_write_ack_event(&found);
        assert_eq!(packet.dest_client_id, COUNTERPARTY_CLIENT);
        assert_ne!(ack.0, Vec::<u8>::new());
        assert!(
            c.query_write_ack_event(&SolanaClientId(CLIENT.into()), PacketSequence(4))
                .await
                .unwrap()
                .is_none()
        );
    }

    // --- message building and sending ---

    struct Deployed {
        ics26: Pubkey,
        ics07: Pubkey,
        am: Pubkey,
        app: Pubkey,
    }

    /// Seed the client, router and app accounts the packet builders resolve.
    fn deployed(client: &str, port: &str) -> (Deployed, FakeRpc) {
        let d = Deployed {
            ics26: Pubkey::new_unique(),
            ics07: Pubkey::new_unique(),
            am: Pubkey::new_unique(),
            app: Pubkey::new_unique(),
        };
        let router = (1u8, (d.am.to_bytes(), None::<[u8; 32]>), false);
        let rpc = FakeRpc::default()
            .with_account(
                Ics26Router::client_pda(client, &d.ics26).0,
                anchor_account("Client", &(1u8, client.to_string(), d.ics07.to_bytes())),
            )
            .with_account(
                Ics26Router::router_state_pda(&d.ics26).0,
                anchor_account("RouterState", &router),
            )
            .with_account(
                Ics26Router::ibc_app_pda(port, &d.ics26).0,
                anchor_account("IBCApp", &(1u8, port.to_string(), d.app.to_bytes())),
            );
        (d, rpc)
    }

    fn assert_packet_ix(msg: &SolanaMessage, d: &Deployed, method: &str, extra: &[AccountMeta]) {
        assert_eq!(msg.instructions.len(), 3);
        let ix = &msg.instructions[2];
        assert_eq!(ix.program_id, d.ics26);
        assert_eq!(
            &ix.data[..8],
            accounts::anchor_instruction_discriminator(method)
        );
        let keys: Vec<_> = ix.accounts.iter().map(|m| m.pubkey).collect();
        assert!(keys.contains(&AccessManager::pda(&d.am).0));
        assert!(keys.contains(&IbcApp::state_pda(&d.app).0));
        assert!(keys.contains(&d.ics07));
        assert!(keys.contains(&Ics07Tendermint::consensus_state_pda(77, &d.ics07).0));
        for meta in extra {
            assert!(ix.accounts.contains(meta), "missing {meta:?}");
        }
    }

    #[tokio::test]
    async fn receive_packet_targets_dest_client_and_port() {
        let (d, rpc) = deployed(COUNTERPARTY_CLIENT, "dst-0");
        let c = chain_with(d.ics26, rpc);
        let msg = c
            .build_receive_packet_message(
                &packet(1),
                SolanaCommitmentProof::None,
                SolanaHeight(77),
                0,
            )
            .await
            .unwrap();
        assert_packet_ix(
            &msg,
            &d,
            "recv_packet",
            &[
                AccountMeta::new(
                    Ics26Router::packet_receipt_pda(COUNTERPARTY_CLIENT, 3, &d.ics26).0,
                    false,
                ),
                AccountMeta::new(c.keypair.pubkey(), true),
            ],
        );
        let decoded: crate::ibc_types::MsgRecvPacket =
            borsh::from_slice(&msg.instructions[2].data[8..]).unwrap();
        assert_eq!(decoded.proof.height, 77);
        assert_eq!(decoded.packet.sequence, 3);
    }

    #[tokio::test]
    async fn ack_packet_targets_source_client_and_carries_ack() {
        let (d, rpc) = deployed(CLIENT, "src-0");
        let c = chain_with(d.ics26, rpc);
        let msg = c
            .build_ack_packet_message(
                &packet(1),
                &SolanaAcknowledgement(vec![9, 9]),
                SolanaCommitmentProof::None,
                SolanaHeight(77),
                0,
            )
            .await
            .unwrap();
        assert_packet_ix(
            &msg,
            &d,
            "ack_packet",
            &[AccountMeta::new(
                Ics26Router::packet_commitment_pda(CLIENT, 3, &d.ics26).0,
                false,
            )],
        );
        let decoded: crate::ibc_types::MsgAckPacket =
            borsh::from_slice(&msg.instructions[2].data[8..]).unwrap();
        assert_eq!(decoded.acknowledgement, vec![9, 9]);
    }

    #[tokio::test]
    async fn timeout_packet_targets_source_client() {
        let (d, rpc) = deployed(CLIENT, "src-0");
        let c = chain_with(d.ics26, rpc);
        let msg = c
            .build_timeout_packet_message(
                &packet(1),
                SolanaCommitmentProof::None,
                SolanaHeight(77),
                0,
            )
            .await
            .unwrap();
        assert_packet_ix(
            &msg,
            &d,
            "timeout_packet",
            &[AccountMeta::new(
                Ics26Router::packet_commitment_pda(CLIENT, 3, &d.ics26).0,
                false,
            )],
        );
    }

    #[tokio::test]
    async fn packet_builders_reject_packets_without_payloads() {
        let c = chain(FakeRpc::default());
        let p = packet(0);
        let none = || SolanaCommitmentProof::None;
        let errs = [
            c.build_receive_packet_message(&p, none(), SolanaHeight(1), 0)
                .await
                .unwrap_err(),
            c.build_ack_packet_message(
                &p,
                &SolanaAcknowledgement(vec![]),
                none(),
                SolanaHeight(1),
                0,
            )
            .await
            .unwrap_err(),
            c.build_timeout_packet_message(&p, none(), SolanaHeight(1), 0)
                .await
                .unwrap_err(),
        ];
        for err in errs {
            assert!(err.to_string().contains("packet has no payloads"), "{err}");
        }
    }

    #[tokio::test]
    async fn packet_builders_require_router_state() {
        let (d, mut rpc) = deployed(COUNTERPARTY_CLIENT, "dst-0");
        rpc.accounts
            .remove(&Ics26Router::router_state_pda(&d.ics26).0);
        let err = chain_with(d.ics26, rpc)
            .build_receive_packet_message(
                &packet(1),
                SolanaCommitmentProof::None,
                SolanaHeight(1),
                0,
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("router state PDA not found"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn send_messages_sends_one_transaction_per_group() {
        let rpc = FakeRpc::default();
        let sent = rpc.sent_txs.clone();
        let c = chain(rpc);
        let messages = vec![
            SolanaMessage {
                instructions: vec![compute_limit(), noop(1), compute_limit(), noop(2)],
            },
            SolanaMessage {
                instructions: vec![noop(3)],
            },
        ];
        c.send_messages(messages).await.unwrap();
        assert_eq!(sent.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn dummy_client_payloads_encode_slot_and_time() {
        let c = chain(FakeRpc {
            slot: Some(7),
            block_time: Some(-1),
            ..FakeRpc::default()
        });
        let create =
            <SolanaChain as ClientPayloadBuilder<SolanaChain>>::build_create_client_payload(&c)
                .await
                .unwrap();
        let SolanaCreateClientPayload::Dummy {
            client_state,
            consensus_state,
        } = create
        else {
            panic!("expected dummy payload");
        };
        assert_eq!(client_state, 7u64.to_le_bytes());
        assert_eq!(consensus_state, 0u64.to_le_bytes());

        let update =
            <SolanaChain as ClientPayloadBuilder<SolanaChain>>::build_update_client_payload(
                &c,
                &SolanaHeight(1),
                &SolanaHeight(7),
                &SolanaClientState(vec![]),
            )
            .await
            .unwrap();
        let SolanaUpdateClientPayload::Dummy { headers } = update else {
            panic!("expected dummy payload");
        };
        let mut expected = 7u64.to_le_bytes().to_vec();
        expected.extend(0u64.to_le_bytes());
        assert_eq!(headers, vec![expected]);
    }

    #[tokio::test]
    async fn unsupported_operations_fail_loudly_or_report_nothing() {
        let c = chain(FakeRpc::default());
        let id = SolanaClientId(CLIENT.into());
        let dummy_create = SolanaCreateClientPayload::Dummy {
            client_state: vec![],
            consensus_state: vec![],
        };
        assert!(c.build_create_client_message(dummy_create).await.is_err());
        assert!(
            c.build_update_client_message(
                &id,
                SolanaUpdateClientPayload::Dummy { headers: vec![] }
            )
            .await
            .is_err()
        );
        assert!(
            c.build_register_counterparty_message(
                &id,
                &id,
                mercury_core::MerklePrefix::ibc_default()
            )
            .await
            .is_err()
        );
        assert!(
            c.build_misbehaviour_message(&id, SolanaMisbehaviourEvidence)
                .await
                .is_err()
        );
        assert!(
            c.check_for_misbehaviour(&id, &vec![], &SolanaClientState(vec![]))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            c.query_consensus_state_heights(&id).await.unwrap(),
            Vec::<SolanaHeight>::new()
        );
        assert!(
            c.query_update_client_header(&id, &SolanaHeight(1))
                .await
                .unwrap()
                .is_none()
        );
    }

    // --- trait accessors ---

    #[test]
    fn chain_types_expose_status_and_config() {
        let c = chain(FakeRpc::default());
        let status = SolanaChainStatus {
            height: SolanaHeight(12),
            timestamp: SolanaTimestamp(34),
        };
        assert_eq!(SolanaChain::chain_status_height(&status), &SolanaHeight(12));
        assert_eq!(
            SolanaChain::chain_status_timestamp(&status),
            &SolanaTimestamp(34)
        );
        assert_eq!(SolanaChain::chain_status_timestamp_secs(&status), 34);
        assert_eq!(c.revision_number(), 0);
        assert_eq!(c.block_time(), Duration::from_millis(400));
        assert_eq!(c.chain_id().to_string(), SolanaChainId.to_string());
        assert_eq!(c.chain_label().name(), "solana");
        assert_eq!(c.chain_label().id(), None);

        let p = packet(1);
        assert_eq!(SolanaChain::packet_sequence(&p), PacketSequence(3));
        assert_eq!(
            SolanaChain::packet_timeout_timestamp(&p),
            TimeoutTimestamp(1_000)
        );
    }

    #[tokio::test]
    async fn send_and_ack_events_round_trip_through_trait_helpers() {
        let ics26 = Pubkey::new_unique();
        let mut rpc = FakeRpc::default();
        rpc.blocks.insert(
            1,
            block(&[tx(&ics26, Some(vec![send_log(9), ack_log(9)]), false)]),
        );
        let events = chain_with(ics26, rpc)
            .query_block_events(&SolanaHeight(1))
            .await
            .unwrap();
        assert_eq!(events.len(), 2);

        let send = SolanaChain::try_extract_send_packet_event(&events[0]).unwrap();
        assert_eq!(
            SolanaChain::packet_from_send_event(&send).sequence,
            PacketSequence(9)
        );
        assert!(SolanaChain::try_extract_write_ack_event(&events[0]).is_none());
        let ack = SolanaChain::try_extract_write_ack_event(&events[1]).unwrap();
        assert_eq!(
            SolanaChain::packet_from_write_ack_event(&ack).0.sequence,
            PacketSequence(9)
        );
    }

    // --- attestation ---

    fn attestation(quorum: u32) -> crate::config::AttestationConfig {
        crate::config::AttestationConfig {
            endpoints: vec!["http://127.0.0.1:1".into(), "http://127.0.0.1:2".into()],
            attestor_addresses: vec![
                format!("0x{}", "11".repeat(20)),
                format!("0x{}", "22".repeat(20)),
            ],
            min_required_sigs: quorum,
            timeout_secs: 1,
        }
    }

    #[tokio::test]
    async fn new_builds_attestor_from_attestation_config() {
        let path = write_keypair(&Keypair::new());
        let mut cfg = config(path.clone());
        cfg.attestation = Some(attestation(2));
        let c = SolanaChain::new(cfg.clone()).unwrap();
        let aggregator = c.attestor.clone().unwrap();
        assert_eq!(aggregator.quorum(), 2);
        assert_eq!(aggregator.attestor_addresses().len(), 2);
        assert!(matches!(
            c.commitment_proof(),
            SolanaCommitmentProof::Attested(_)
        ));
        // Attested clients only hold consensus states at heights they were updated to.
        assert_eq!(
            <SolanaChain as ClientPayloadBuilder<SolanaChain>>::proof_height_without_update(
                &c,
                &SolanaHeight(90),
                &SolanaHeight(100),
            ),
            SolanaHeight(100)
        );

        cfg.attestation = Some(attestation(3));
        let err = SolanaChain::new(cfg).unwrap_err();
        assert!(err.to_string().contains("exceeds"), "{err}");
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn attested_create_payload_fails_without_a_reachable_quorum() {
        let path = write_keypair(&Keypair::new());
        let mut cfg = config(path.clone());
        cfg.attestation = Some(attestation(1));
        let c = SolanaChain::new(cfg).unwrap();
        // Never falls back to the dummy payload, even though the RPC would answer.
        let c = SolanaChain {
            rpc: FakeRpc {
                slot: Some(1),
                block_time: Some(1),
                ..FakeRpc::default()
            }
            .into_client(),
            ..c
        };
        assert!(
            <SolanaChain as ClientPayloadBuilder<SolanaChain>>::build_create_client_payload(&c)
                .await
                .is_err()
        );
        std::fs::remove_file(path).unwrap();
    }

    // --- address lookup table ---

    // Address lookup table program account layout: bincode `ProgramState::LookupTable`
    // (u32 tag 1) + `LookupTableMeta` padded to 56 bytes, then the addresses.
    fn alt_account(authority: &Pubkey, addresses: &[Pubkey]) -> Vec<u8> {
        let mut data = 1u32.to_le_bytes().to_vec();
        data.extend(u64::MAX.to_le_bytes()); // deactivation_slot: active
        data.extend(0u64.to_le_bytes()); // last_extended_slot
        data.push(0); // last_extended_slot_start_index
        data.push(1); // Some(authority)
        data.extend(authority.to_bytes());
        data.extend([0u8; 2]); // padding
        assert_eq!(data.len(), 56);
        for a in addresses {
            data.extend(a.to_bytes());
        }
        data
    }

    #[tokio::test]
    async fn load_alt_cache_reads_table_and_shrinks_transactions() {
        let alt = Pubkey::new_unique();
        let shared: Vec<Pubkey> = (0..4).map(|_| Pubkey::new_unique()).collect();
        let rpc = FakeRpc::default().with_account(alt, alt_account(&Pubkey::new_unique(), &shared));
        let sent = rpc.sent_txs.clone();
        let mut c = chain(rpc);
        c.config.alt_address = Some(alt.to_string());
        c.load_alt_cache().await.unwrap();
        let cached = c.alt_cache.clone().unwrap();
        assert_eq!(cached.key, alt);
        assert_eq!(cached.addresses, shared);

        let ix = Instruction::new_with_bytes(
            Pubkey::new_unique(),
            &[1],
            shared
                .iter()
                .map(|k| AccountMeta::new_readonly(*k, false))
                .collect(),
        );
        let msg = || SolanaMessage {
            instructions: vec![ix.clone()],
        };
        c.send_messages(vec![msg()]).await.unwrap();
        c.alt_cache = None;
        c.send_messages(vec![msg()]).await.unwrap();
        let sizes: Vec<usize> = sent.lock().unwrap().iter().map(Vec::len).collect();
        // Four 32-byte keys become 1-byte indexes behind a 32-byte table key.
        assert!(sizes[0] < sizes[1], "{sizes:?}");

        c.config.alt_address = Some("not-a-pubkey".into());
        let err = c.load_alt_cache().await.unwrap_err();
        assert!(err.to_string().contains("invalid alt_address"), "{err}");
    }
}
