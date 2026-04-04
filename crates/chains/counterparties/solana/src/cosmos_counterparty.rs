use std::time::Duration;

use async_trait::async_trait;
use tendermint::block::Height as TmHeight;

use mercury_chain_traits::builders::{
    ClientMessageBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder, PacketMessageBuilder,
    UpdateClientOutput,
};
use mercury_chain_traits::queries::{ClientQuery, MisbehaviourQuery};
use mercury_chain_traits::types::ChainTypes;
use mercury_core::error::Result;
use solana_sdk::signer::Signer;

use mercury_cosmos::builders::{CosmosCreateClientPayload, CosmosUpdateClientPayload};
use mercury_cosmos::chain::CosmosChain;
use mercury_cosmos::client_types::CosmosClientState;
use mercury_cosmos::keys::{CosmosSigner, Secp256k1KeyPair};
use mercury_cosmos::types::{CosmosPacket, MerkleProof, PacketAcknowledgement};

use mercury_solana::accounts::{
    self, Ics26Router, OnChainRouterState, fetch_account, resolve_ics07_program_id,
};
use mercury_solana::chain::{SolanaChain, SolanaMisbehaviourEvidence};
use mercury_solana::instructions;
use mercury_solana::instructions::chunking;
use mercury_solana::types::{
    SolanaClientId, SolanaClientState, SolanaConsensusState, SolanaHeight, SolanaMessage,
    SolanaPacket,
};

use crate::wrapper::SolanaAdapter;

fn cosmos_packet_to_ibc_parts(
    packet: &CosmosPacket,
) -> (
    mercury_solana::ibc_types::Packet,
    Vec<mercury_solana::ibc_types::PayloadMetadata>,
) {
    let ibc_packet = mercury_solana::ibc_types::Packet {
        sequence: packet.sequence.0,
        source_client: packet.source_client_id.0.clone(),
        dest_client: packet.dest_client_id.0.clone(),
        timeout_timestamp: packet.timeout_timestamp.0,
        payloads: packet
            .payloads
            .iter()
            .map(|p| mercury_solana::ibc_types::Payload {
                source_port: p.source_port.0.clone(),
                dest_port: p.dest_port.0.clone(),
                version: p.version.clone(),
                encoding: p.encoding.clone(),
                value: p.data.clone(),
            })
            .collect(),
    };
    let metas = packet
        .payloads
        .iter()
        .map(|p| mercury_solana::ibc_types::PayloadMetadata {
            source_port: p.source_port.0.clone(),
            dest_port: p.dest_port.0.clone(),
            version: p.version.clone(),
            encoding: p.encoding.clone(),
            total_chunks: 0,
        })
        .collect();
    (ibc_packet, metas)
}

async fn resolve_access_manager(chain: &SolanaChain) -> Result<solana_sdk::pubkey::Pubkey> {
    let (router_pda, _) = Ics26Router::router_state_pda(&chain.ics26_program_id);
    let router: OnChainRouterState = fetch_account(&chain.rpc, &router_pda)
        .await?
        .ok_or_else(|| eyre::eyre!("router state PDA not found"))?;
    Ok(router.access_manager)
}

/// Build chunked packet message: uploads payload/proof chunks, then sends final packet tx.
/// Returns (chunk upload messages, final packet message, cleanup message).
struct ChunkedPacketOutput {
    chunk_messages: Vec<SolanaMessage>,
    packet_message: SolanaMessage,
    cleanup_message: Option<SolanaMessage>,
}

struct ChunkedPacketParams<'a> {
    ics26_program_id: &'a solana_sdk::pubkey::Pubkey,
    payer: &'a solana_sdk::pubkey::Pubkey,
    client_id: &'a str,
    sequence: u64,
    ibc_packet: &'a mercury_solana::ibc_types::Packet,
    payload_metas: &'a mut [mercury_solana::ibc_types::PayloadMetadata],
    proof_bytes: &'a [u8],
    proof_height: u64,
}

fn build_chunked_packet_message(
    params: ChunkedPacketParams<'_>,
    build_packet_ix: impl FnOnce(
        &mercury_solana::ibc_types::Packet,
        &[mercury_solana::ibc_types::PayloadMetadata],
        mercury_solana::ibc_types::ProofMetadata,
    ) -> eyre::Result<SolanaMessage>,
) -> eyre::Result<ChunkedPacketOutput> {
    let ChunkedPacketParams {
        ics26_program_id,
        payer,
        client_id,
        sequence,
        ibc_packet,
        payload_metas,
        proof_bytes,
        proof_height,
    } = params;
    let mut chunk_messages = Vec::new();
    let mut payload_chunk_counts: Vec<u8> = Vec::new();

    for (payload_idx, payload) in ibc_packet.payloads.iter().enumerate() {
        let p_idx = u8::try_from(payload_idx)
            .map_err(|_| eyre::eyre!("payload index {payload_idx} exceeds u8::MAX"))?;

        if chunking::needs_chunking(&payload.value) {
            let (ixs, _pdas) = chunking::chunk_payload(
                ics26_program_id,
                payer,
                client_id,
                sequence,
                p_idx,
                &payload.value,
            )?;
            let chunk_count = u8::try_from(ixs.len())
                .map_err(|_| eyre::eyre!("payload chunk count exceeds u8::MAX"))?;
            payload_metas[payload_idx].total_chunks = chunk_count;
            payload_chunk_counts.push(chunk_count);
            for ix in ixs {
                chunk_messages.push(SolanaMessage {
                    instructions: instructions::with_compute_budget(ix),
                });
            }
        } else {
            payload_chunk_counts.push(0);
        }
    }

    let mut proof_chunk_count: u8 = 0;
    if chunking::needs_chunking(proof_bytes) {
        let (ixs, _pdas) =
            chunking::chunk_proof(ics26_program_id, payer, client_id, sequence, proof_bytes)?;
        proof_chunk_count = u8::try_from(ixs.len())
            .map_err(|_| eyre::eyre!("proof chunk count exceeds u8::MAX"))?;
        for ix in ixs {
            chunk_messages.push(SolanaMessage {
                instructions: instructions::with_compute_budget(ix),
            });
        }
    }

    let proof_meta = mercury_solana::ibc_types::ProofMetadata {
        height: proof_height,
        total_chunks: proof_chunk_count,
    };

    let packet_message = build_packet_ix(ibc_packet, payload_metas, proof_meta)?;

    let has_chunks = !chunk_messages.is_empty();
    let cleanup_message = if has_chunks {
        let mut cleanup_ixs = Vec::new();
        let has_payload_chunks = payload_chunk_counts.iter().any(|&c| c > 0);
        if has_payload_chunks {
            let payload_count = u8::try_from(payload_chunk_counts.len())
                .map_err(|_| eyre::eyre!("payload count exceeds u8::MAX"))?;
            cleanup_ixs.extend(chunking::cleanup_payload_chunks(
                ics26_program_id,
                payer,
                client_id,
                sequence,
                payload_count,
                &payload_chunk_counts,
            )?);
        }
        if proof_chunk_count > 0 {
            cleanup_ixs.extend(chunking::cleanup_proof_chunks(
                ics26_program_id,
                payer,
                client_id,
                sequence,
                proof_chunk_count,
            )?);
        }
        wrap_cleanup_message(cleanup_ixs)
    } else {
        None
    };

    Ok(ChunkedPacketOutput {
        chunk_messages,
        packet_message,
        cleanup_message,
    })
}

/// Wrap cleanup instructions with a compute budget prefix.
fn wrap_cleanup_message(
    mut ixs: Vec<solana_sdk::instruction::Instruction>,
) -> Option<SolanaMessage> {
    if ixs.is_empty() {
        return None;
    }
    let first = ixs.remove(0);
    let mut wrapped = instructions::with_compute_budget(first);
    wrapped.extend(ixs);
    Some(SolanaMessage {
        instructions: wrapped,
    })
}

/// Flatten chunked output into a single `SolanaMessage` whose instructions
/// will be split into separate transactions by `send_messages`.
fn flatten_chunked_output(output: ChunkedPacketOutput) -> SolanaMessage {
    if output.chunk_messages.is_empty() {
        return output.packet_message;
    }
    let all_instructions: Vec<_> = output
        .chunk_messages
        .into_iter()
        .chain(std::iter::once(output.packet_message))
        .chain(output.cleanup_message)
        .flat_map(|m| m.instructions)
        .collect();
    SolanaMessage {
        instructions: all_instructions,
    }
}

#[async_trait]
impl PacketMessageBuilder<CosmosChain<Secp256k1KeyPair>> for SolanaAdapter {
    async fn build_receive_packet_message(
        &self,
        packet: &CosmosPacket,
        proof: MerkleProof,
        proof_height: TmHeight,
        _revision: u64,
    ) -> Result<SolanaMessage> {
        let chain = &self.0;
        let dest_client_id = &packet.dest_client_id.0;
        let dest_port = &packet
            .payloads
            .first()
            .ok_or_else(|| eyre::eyre!("packet has no payloads"))?
            .dest_port
            .0;
        let sequence = packet.sequence.0;
        let payer = chain.keypair.pubkey();

        let ics07 =
            resolve_ics07_program_id(&chain.rpc, dest_client_id, &chain.ics26_program_id).await?;
        let access_mgr = resolve_access_manager(chain).await?;
        let app_program =
            accounts::resolve_app_program_id(&chain.rpc, dest_port, &chain.ics26_program_id)
                .await?;

        let (ibc_packet, mut payload_metas) = cosmos_packet_to_ibc_parts(packet);
        let proof_bytes = &proof.proof_bytes;
        let height_val = proof_height.value();

        let output = build_chunked_packet_message(
            ChunkedPacketParams {
                ics26_program_id: &chain.ics26_program_id,
                payer: &payer,
                client_id: dest_client_id,
                sequence,
                ibc_packet: &ibc_packet,
                payload_metas: &mut payload_metas,
                proof_bytes,
                proof_height: height_val,
            },
            |pkt, metas, proof_meta| {
                let msg = mercury_solana::ibc_types::MsgRecvPacket {
                    packet: pkt.clone(),
                    payloads: metas.to_vec(),
                    proof: proof_meta,
                };
                let params = instructions::PacketParams {
                    ics26_program_id: &chain.ics26_program_id,
                    payer: &payer,
                    client_id: dest_client_id,
                    port: dest_port,
                    sequence,
                    ics07_program_id: &ics07,
                    consensus_height: height_val,
                    access_manager_program_id: &access_mgr,
                    app_program_id: &app_program,
                };
                let ix = instructions::recv_packet(&params, &msg)?;
                Ok(SolanaMessage {
                    instructions: instructions::with_compute_budget(ix),
                })
            },
        )?;

        Ok(flatten_chunked_output(output))
    }

    async fn build_ack_packet_message(
        &self,
        packet: &CosmosPacket,
        ack: &PacketAcknowledgement,
        proof: MerkleProof,
        proof_height: TmHeight,
        _revision: u64,
    ) -> Result<SolanaMessage> {
        let chain = &self.0;
        let source_client_id = &packet.source_client_id.0;
        let source_port = &packet
            .payloads
            .first()
            .ok_or_else(|| eyre::eyre!("packet has no payloads"))?
            .source_port
            .0;
        let sequence = packet.sequence.0;
        let payer = chain.keypair.pubkey();

        let ics07 =
            resolve_ics07_program_id(&chain.rpc, source_client_id, &chain.ics26_program_id).await?;
        let access_mgr = resolve_access_manager(chain).await?;
        let app_program =
            accounts::resolve_app_program_id(&chain.rpc, source_port, &chain.ics26_program_id)
                .await?;

        let (ibc_packet, mut payload_metas) = cosmos_packet_to_ibc_parts(packet);
        let proof_bytes = &proof.proof_bytes;
        let height_val = proof_height.value();
        let ack_bytes = ack.0.clone();

        let output = build_chunked_packet_message(
            ChunkedPacketParams {
                ics26_program_id: &chain.ics26_program_id,
                payer: &payer,
                client_id: source_client_id,
                sequence,
                ibc_packet: &ibc_packet,
                payload_metas: &mut payload_metas,
                proof_bytes,
                proof_height: height_val,
            },
            |pkt, metas, proof_meta| {
                let msg = mercury_solana::ibc_types::MsgAckPacket {
                    packet: pkt.clone(),
                    payloads: metas.to_vec(),
                    acknowledgement: ack_bytes.clone(),
                    proof: proof_meta,
                };
                let params = instructions::PacketParams {
                    ics26_program_id: &chain.ics26_program_id,
                    payer: &payer,
                    client_id: source_client_id,
                    port: source_port,
                    sequence,
                    ics07_program_id: &ics07,
                    consensus_height: height_val,
                    access_manager_program_id: &access_mgr,
                    app_program_id: &app_program,
                };
                let ix = instructions::ack_packet(&params, &msg)?;
                Ok(SolanaMessage {
                    instructions: instructions::with_compute_budget(ix),
                })
            },
        )?;

        Ok(flatten_chunked_output(output))
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &SolanaPacket,
        proof: MerkleProof,
        proof_height: TmHeight,
        _revision: u64,
    ) -> Result<SolanaMessage> {
        let chain = &self.0;
        let source_client_id = &packet.source_client_id;
        let source_port = &packet
            .payloads
            .first()
            .ok_or_else(|| eyre::eyre!("packet has no payloads"))?
            .source_port
            .0;
        let sequence = packet.sequence.0;
        let payer = chain.keypair.pubkey();

        let ics07 =
            resolve_ics07_program_id(&chain.rpc, source_client_id, &chain.ics26_program_id).await?;
        let access_mgr = resolve_access_manager(chain).await?;
        let app_program =
            accounts::resolve_app_program_id(&chain.rpc, source_port, &chain.ics26_program_id)
                .await?;

        let (ibc_packet, mut payload_metas) = packet.to_ibc_parts();
        let proof_bytes = &proof.proof_bytes;
        let height_val = proof_height.value();

        let output = build_chunked_packet_message(
            ChunkedPacketParams {
                ics26_program_id: &chain.ics26_program_id,
                payer: &payer,
                client_id: source_client_id,
                sequence,
                ibc_packet: &ibc_packet,
                payload_metas: &mut payload_metas,
                proof_bytes,
                proof_height: height_val,
            },
            |pkt, metas, proof_meta| {
                let msg = mercury_solana::ibc_types::MsgTimeoutPacket {
                    packet: pkt.clone(),
                    payloads: metas.to_vec(),
                    proof: proof_meta,
                };
                let params = instructions::PacketParams {
                    ics26_program_id: &chain.ics26_program_id,
                    payer: &payer,
                    client_id: source_client_id,
                    port: source_port,
                    sequence,
                    ics07_program_id: &ics07,
                    consensus_height: height_val,
                    access_manager_program_id: &access_mgr,
                    app_program_id: &app_program,
                };
                let ix = instructions::timeout_packet(&params, &msg)?;
                Ok(SolanaMessage {
                    instructions: instructions::with_compute_budget(ix),
                })
            },
        )?;

        Ok(flatten_chunked_output(output))
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientMessageBuilder<CosmosChain<S>> for SolanaAdapter {
    type CreateClientPayload = CosmosCreateClientPayload;
    type UpdateClientPayload = CosmosUpdateClientPayload;

    async fn build_create_client_message(
        &self,
        payload: CosmosCreateClientPayload,
    ) -> Result<SolanaMessage> {
        use ibc_client_tendermint::types::ClientState as TmClientState;
        use ibc_client_tendermint::types::ConsensusState as TmConsensusState;

        let chain = &self.0;
        let ics07_program_id = chain
            .ics07_program_id
            .ok_or_else(|| eyre::eyre!("ics07_program_id not configured"))?;

        let access_mgr = resolve_access_manager(chain).await?;
        let payer = chain.keypair.pubkey();

        let tm_client_state: TmClientState = payload
            .client_state
            .try_into()
            .map_err(|e| eyre::eyre!("failed to decode client state: {e}"))?;
        let tm_consensus_state: TmConsensusState = payload
            .consensus_state
            .try_into()
            .map_err(|e| eyre::eyre!("failed to decode consensus state: {e}"))?;

        let client_state = mercury_solana::ibc_types::ClientState {
            chain_id: tm_client_state.chain_id.to_string(),
            trust_level_numerator: tm_client_state.trust_level.numerator(),
            trust_level_denominator: tm_client_state.trust_level.denominator(),
            trusting_period: tm_client_state.trusting_period.as_secs(),
            unbonding_period: tm_client_state.unbonding_period.as_secs(),
            max_clock_drift: tm_client_state.max_clock_drift.as_secs(),
            frozen_height: mercury_solana::ibc_types::IbcHeight {
                revision_number: 0,
                revision_height: 0,
            },
            latest_height: mercury_solana::ibc_types::IbcHeight {
                revision_number: tm_client_state.latest_height.revision_number(),
                revision_height: tm_client_state.latest_height.revision_height(),
            },
        };

        let root_bytes: [u8; 32] = tm_consensus_state
            .root
            .as_bytes()
            .try_into()
            .map_err(|_| eyre::eyre!("consensus state root is not 32 bytes"))?;
        let next_val_hash: [u8; 32] = tm_consensus_state
            .next_validators_hash
            .as_bytes()
            .try_into()
            .map_err(|_| eyre::eyre!("next_validators_hash is not 32 bytes"))?;

        let consensus_state = mercury_solana::ibc_types::ConsensusState {
            timestamp: tm_consensus_state
                .timestamp
                .unix_timestamp()
                .cast_unsigned(),
            root: root_bytes,
            next_validators_hash: next_val_hash,
        };

        let client_id = "07-tendermint-0";

        let counterparty_info = mercury_solana::ibc_types::CounterpartyInfo {
            client_id: String::new(),
            merkle_prefix: vec![b"ibc".to_vec()],
        };

        let add_client_ix = mercury_solana::instructions::client::add_client(
            &chain.ics26_program_id,
            &payer,
            client_id,
            counterparty_info,
            &ics07_program_id,
            &access_mgr,
        )?;

        let init_ix = mercury_solana::instructions::client::initialize_ics07(
            &ics07_program_id,
            &payer,
            &client_state,
            &consensus_state,
            &access_mgr,
        )?;

        let mut instructions = mercury_solana::instructions::with_compute_budget(add_client_ix);
        instructions.extend(mercury_solana::instructions::with_compute_budget(init_ix));

        Ok(SolanaMessage { instructions })
    }

    async fn build_update_client_message(
        &self,
        client_id: &SolanaClientId,
        payload: CosmosUpdateClientPayload,
    ) -> Result<UpdateClientOutput<SolanaMessage>> {
        use mercury_solana::accounts::{Ics07Tendermint, resolve_ics07_program_id};
        use mercury_solana::instructions::chunking;
        use mercury_solana::instructions::signatures;

        let chain = &self.0;
        let payer = chain.keypair.pubkey();
        let ics07 =
            resolve_ics07_program_id(&chain.rpc, &client_id.0, &chain.ics26_program_id).await?;
        let access_mgr = resolve_access_manager(chain).await?;

        let header_any = payload
            .headers
            .first()
            .ok_or_else(|| eyre::eyre!("update_client payload has no headers"))?;
        let header: ibc_client_tendermint::types::Header = header_any
            .clone()
            .try_into()
            .map_err(|e| eyre::eyre!("failed to decode tendermint header: {e}"))?;

        let target_height = header.height().revision_height();
        let trusted_height = header.trusted_height.revision_height();

        // Threshold is checked against the full commit (not the minimal set)
        // so devnet (few validators) verifies inline while mainnet pre-verifies.
        let all_signatures = signatures::extract_signatures_from_header(&header);
        let use_pre_verify = all_signatures.len() > chain.config.skip_pre_verify_threshold();
        let selected_sigs = signatures::select_minimal_signatures(&header, &all_signatures);

        let header_any: ibc_proto::google::protobuf::Any = header.into();
        let header_bytes = header_any.value;

        let mut messages = Vec::new();

        let header_chunks = chunking::chunk_data(&header_bytes);
        let chunk_count = u8::try_from(header_chunks.len())
            .map_err(|_| eyre::eyre!("header chunk count exceeds u8::MAX"))?;
        let mut header_chunk_pdas = Vec::with_capacity(header_chunks.len());
        for (i, chunk) in header_chunks.into_iter().enumerate() {
            let chunk_idx = u8::try_from(i)
                .map_err(|_| eyre::eyre!("header chunk index {i} exceeds u8::MAX"))?;
            let ix =
                signatures::upload_header_chunk(&ics07, &payer, target_height, chunk_idx, chunk)?;
            messages.push(SolanaMessage {
                instructions: instructions::with_compute_budget(ix),
            });
            let (pda, _) =
                Ics07Tendermint::header_chunk_pda(&payer, target_height, chunk_idx, &ics07);
            header_chunk_pdas.push(pda);
        }

        let sig_verify_pdas = if use_pre_verify {
            let mut pdas = Vec::with_capacity(selected_sigs.len());
            for sig in &selected_sigs {
                let ixs = signatures::pre_verify_signature_instructions(
                    &ics07,
                    &payer,
                    sig,
                    &access_mgr,
                )?;
                messages.push(SolanaMessage { instructions: ixs });

                let (pda, _) = Ics07Tendermint::sig_verify_pda(&sig.signature_hash, &ics07);
                pdas.push(pda);
            }
            pdas
        } else {
            Vec::new()
        };

        let assemble_ixs = signatures::assemble_and_update_client(
            &ics07,
            &payer,
            trusted_height,
            target_height,
            &header_chunk_pdas,
            &sig_verify_pdas,
            &access_mgr,
        )?;

        messages.push(SolanaMessage {
            instructions: assemble_ixs,
        });

        let cleanup_ixs =
            signatures::cleanup_header_chunks(&ics07, &payer, target_height, chunk_count)?;
        if let Some(msg) = wrap_cleanup_message(cleanup_ixs) {
            messages.push(msg);
        }

        if !sig_verify_pdas.is_empty() {
            let sig_cleanup_ixs =
                signatures::cleanup_sig_verify_pdas(&ics07, &payer, &sig_verify_pdas)?;
            if let Some(msg) = wrap_cleanup_message(sig_cleanup_ixs) {
                messages.push(msg);
            }
        }

        Ok(UpdateClientOutput {
            messages,
            membership_proof: None,
        })
    }

    async fn build_register_counterparty_message(
        &self,
        _client_id: &SolanaClientId,
        _counterparty_client_id: &<CosmosChain<S> as ChainTypes>::ClientId,
        _counterparty_merkle_prefix: mercury_core::MerklePrefix,
    ) -> Result<SolanaMessage> {
        eyre::bail!(
            "register_counterparty is not a separate step on Solana — \
             counterparty info is set during add_client"
        )
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientQuery<CosmosChain<S>> for SolanaAdapter {
    async fn query_client_state(
        &self,
        client_id: &SolanaClientId,
        height: &SolanaHeight,
    ) -> Result<SolanaClientState> {
        <SolanaChain as ClientQuery<SolanaChain>>::query_client_state(&self.0, client_id, height)
            .await
    }

    async fn query_consensus_state(
        &self,
        client_id: &SolanaClientId,
        consensus_height: &TmHeight,
        query_height: &SolanaHeight,
    ) -> Result<SolanaConsensusState> {
        let solana_height = SolanaHeight(consensus_height.value());
        <SolanaChain as ClientQuery<SolanaChain>>::query_consensus_state(
            &self.0,
            client_id,
            &solana_height,
            query_height,
        )
        .await
    }

    fn trusting_period(client_state: &SolanaClientState) -> Option<Duration> {
        <SolanaChain as ClientQuery<SolanaChain>>::trusting_period(client_state)
    }

    fn client_latest_height(client_state: &SolanaClientState) -> TmHeight {
        let h = <SolanaChain as ClientQuery<SolanaChain>>::client_latest_height(client_state);
        TmHeight::try_from(h.0).expect("height conversion")
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourDetector<CosmosChain<S>> for SolanaAdapter {
    type UpdateHeader = ibc_client_tendermint::types::Header;
    type MisbehaviourEvidence = SolanaMisbehaviourEvidence;
    type CounterpartyClientState = CosmosClientState;

    async fn check_for_misbehaviour(
        &self,
        _client_id: &<CosmosChain<S> as ChainTypes>::ClientId,
        _update_header: &Self::UpdateHeader,
        _client_state: &Self::CounterpartyClientState,
    ) -> Result<Option<Self::MisbehaviourEvidence>> {
        tracing::debug!("misbehaviour detection not yet implemented for Cosmos-on-Solana");
        Ok(None)
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourQuery<CosmosChain<S>> for SolanaAdapter {
    type CounterpartyUpdateHeader = ibc_client_tendermint::types::Header;

    async fn query_consensus_state_heights(
        &self,
        _client_id: &SolanaClientId,
    ) -> Result<Vec<TmHeight>> {
        Ok(Vec::new())
    }

    async fn query_update_client_header(
        &self,
        _client_id: &SolanaClientId,
        _consensus_height: &TmHeight,
    ) -> Result<Option<ibc_client_tendermint::types::Header>> {
        Ok(None)
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourMessageBuilder<CosmosChain<S>> for SolanaAdapter {
    type MisbehaviourEvidence = mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence;

    async fn build_misbehaviour_message(
        &self,
        _client_id: &SolanaClientId,
        _evidence: mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence,
    ) -> Result<SolanaMessage> {
        eyre::bail!("misbehaviour submission not yet implemented for Cosmos-on-Solana")
    }
}
