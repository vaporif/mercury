use std::sync::Arc;
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
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::signer::keypair::Keypair;

use crate::accounts::{
    self, Ics07Tendermint, Ics26Router, OnChainClientSequence, OnChainClientState,
    OnChainCommitment, deserialize_anchor_account, fetch_account, resolve_ics07_program_id,
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

#[derive(Clone, Debug)]
pub struct SolanaChain {
    pub config: SolanaChainConfig,
    pub rpc: SolanaRpcClient,
    pub keypair: Arc<Keypair>,
    pub ics26_program_id: Pubkey,
    pub ics07_program_id: Option<Pubkey>,
    pub alt_cache: Option<solana_message::AddressLookupTableAccount>,
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
        let label = mercury_core::ChainLabel::new("solana");
        Ok(Self {
            config,
            rpc,
            keypair,
            ics26_program_id,
            ics07_program_id,
            alt_cache: None,
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

    async fn scan_sequences(
        &self,
        client_id: &SolanaClientId,
        pda_fn: impl Fn(&str, u64, &Pubkey) -> (Pubkey, u8),
    ) -> Result<Vec<PacketSequence>> {
        let (seq_pda, _) = Ics26Router::client_sequence_pda(&client_id.0, &self.ics26_program_id);
        let seq: OnChainClientSequence = fetch_account(&self.rpc, &seq_pda)
            .await?
            .ok_or_else(|| eyre::eyre!("client sequence PDA not found for {client_id}"))?;

        let mut sequences = Vec::new();
        for s in 1..seq.next_sequence_send {
            let (pda, _) = pda_fn(&client_id.0, s, &self.ics26_program_id);
            if self.rpc.get_account(&pda).await?.is_some() {
                sequences.push(PacketSequence(s));
            }
        }
        Ok(sequences)
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
        let proof = SolanaCommitmentProof(Vec::new());
        Ok((
            commitment.map(|c| SolanaPacketCommitment(c.value.to_vec())),
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
        let proof = SolanaCommitmentProof(Vec::new());
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
        let proof = SolanaCommitmentProof(Vec::new());
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
        self.scan_sequences(client_id, |cid, seq, prog| {
            Ics26Router::packet_commitment_pda(cid, seq, prog)
        })
        .await
    }

    async fn query_ack_sequences(
        &self,
        client_id: &Self::ClientId,
        _height: &Self::Height,
    ) -> Result<Vec<PacketSequence>> {
        self.scan_sequences(client_id, |cid, seq, prog| {
            Ics26Router::packet_ack_pda(cid, seq, prog)
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
        let slot = self.rpc.get_slot().await?;
        let block_time = self.rpc.get_block_time(slot).await?;
        Ok(SolanaCreateClientPayload {
            client_state: slot.to_le_bytes().to_vec(),
            consensus_state: block_time.max(0).cast_unsigned().to_le_bytes().to_vec(),
        })
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
        let slot = self.rpc.get_slot().await?;
        let block_time = self.rpc.get_block_time(slot).await?;
        let mut header = Vec::with_capacity(16);
        header.extend_from_slice(&slot.to_le_bytes());
        header.extend_from_slice(&block_time.max(0).cast_unsigned().to_le_bytes());
        Ok(SolanaUpdateClientPayload {
            headers: vec![header],
        })
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

        let (ibc_packet, payload_metas) = packet.to_ibc_parts();

        let msg = crate::ibc_types::MsgRecvPacket {
            packet: ibc_packet,
            payloads: payload_metas,
            proof: crate::ibc_types::ProofMetadata {
                height: proof_height.0,
                total_chunks: 0,
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
            access_manager_program_id: &router.access_manager,
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

        let (ibc_packet, payload_metas) = packet.to_ibc_parts();

        let msg = crate::ibc_types::MsgAckPacket {
            packet: ibc_packet,
            payloads: payload_metas,
            acknowledgement: ack.0.clone(),
            proof: crate::ibc_types::ProofMetadata {
                height: proof_height.0,
                total_chunks: 0,
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
            access_manager_program_id: &router.access_manager,
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

        let (ibc_packet, payload_metas) = packet.to_ibc_parts();

        let msg = crate::ibc_types::MsgTimeoutPacket {
            packet: ibc_packet,
            payloads: payload_metas,
            proof: crate::ibc_types::ProofMetadata {
                height: proof_height.0,
                total_chunks: 0,
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
            access_manager_program_id: &router.access_manager,
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
