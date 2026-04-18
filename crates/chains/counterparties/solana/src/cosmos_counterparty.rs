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

const PAYLOAD_INLINE_THRESHOLD: usize = 300;

fn cosmos_packet_to_msg_packet(packet: &CosmosPacket) -> mercury_solana::ibc_types::MsgPacket {
    mercury_solana::ibc_types::MsgPacket {
        sequence: packet.sequence.0,
        source_client: packet.source_client_id.0.clone(),
        dest_client: packet.dest_client_id.0.clone(),
        timeout_timestamp: packet.timeout_timestamp.0,
        payloads: packet
            .payloads
            .iter()
            .map(|p| mercury_solana::ibc_types::MsgPayload {
                source_port: p.source_port.0.clone(),
                dest_port: p.dest_port.0.clone(),
                version: p.version.clone(),
                encoding: p.encoding.clone(),
                data: mercury_solana::ibc_types::Delivery::Inline {
                    data: p.data.clone(),
                },
            })
            .collect(),
    }
}

async fn resolve_access_manager(chain: &SolanaChain) -> Result<solana_sdk::pubkey::Pubkey> {
    let (router_pda, _) = Ics26Router::router_state_pda(&chain.ics26_program_id);
    let router: OnChainRouterState = fetch_account(&chain.rpc, &router_pda)
        .await?
        .ok_or_else(|| eyre::eyre!("router state PDA not found"))?;
    Ok(router.am_state.access_manager)
}

struct ChunkedPacketOutput {
    chunk_messages: Vec<SolanaMessage>,
    packet_message: SolanaMessage,
    cleanup_message: Option<SolanaMessage>,
}

struct ChunkedPacketParams<'a> {
    ics26_program_id: &'a solana_sdk::pubkey::Pubkey,
    access_manager_program_id: &'a solana_sdk::pubkey::Pubkey,
    payer: &'a solana_sdk::pubkey::Pubkey,
    client_id: &'a str,
    sequence: u64,
    msg_packet: &'a mut mercury_solana::ibc_types::MsgPacket,
    proof_bytes: &'a [u8],
    proof_height: u64,
}

fn build_chunked_packet_message(
    params: ChunkedPacketParams<'_>,
    build_packet_ix: impl FnOnce(
        &mercury_solana::ibc_types::MsgPacket,
        mercury_solana::ibc_types::MsgProof,
        Vec<solana_sdk::instruction::AccountMeta>,
    ) -> eyre::Result<SolanaMessage>,
) -> eyre::Result<ChunkedPacketOutput> {
    let ChunkedPacketParams {
        ics26_program_id,
        access_manager_program_id,
        payer,
        client_id,
        sequence,
        msg_packet,
        proof_bytes,
        proof_height,
    } = params;

    let chunk_ctx = chunking::ChunkContext {
        ics26_program_id,
        payer,
        client_id,
        sequence,
        access_manager_program_id,
    };

    let mut chunk_messages = Vec::new();
    let mut payload_chunk_counts: Vec<u8> = Vec::new();
    let mut chunk_account_metas: Vec<solana_sdk::instruction::AccountMeta> = Vec::new();

    let total_payload_size: usize = msg_packet
        .payloads
        .iter()
        .map(|p| match &p.data {
            mercury_solana::ibc_types::Delivery::Inline { data } => data.len(),
            mercury_solana::ibc_types::Delivery::Chunked { .. } => 0,
        })
        .sum();
    let force_chunk_payloads = total_payload_size >= PAYLOAD_INLINE_THRESHOLD;

    for (payload_idx, payload) in msg_packet.payloads.iter_mut().enumerate() {
        let p_idx = u8::try_from(payload_idx)
            .map_err(|_| eyre::eyre!("payload index {payload_idx} exceeds u8::MAX"))?;

        let payload_data = match &payload.data {
            mercury_solana::ibc_types::Delivery::Inline { data } => data.clone(),
            mercury_solana::ibc_types::Delivery::Chunked { .. } => {
                payload_chunk_counts.push(payload.data.total_chunks());
                continue;
            }
        };

        if force_chunk_payloads || chunking::needs_chunking(&payload_data) {
            let (ixs, pdas) = chunking::chunk_payload(&chunk_ctx, p_idx, &payload_data)?;
            let chunk_count = u8::try_from(ixs.len())
                .map_err(|_| eyre::eyre!("payload chunk count exceeds u8::MAX"))?;
            payload.data = mercury_solana::ibc_types::Delivery::Chunked {
                total_chunks: chunk_count,
            };
            payload_chunk_counts.push(chunk_count);
            for pda in &pdas {
                chunk_account_metas.push(solana_sdk::instruction::AccountMeta::new(*pda, false));
            }
            for ix in ixs {
                chunk_messages.push(SolanaMessage {
                    instructions: vec![ix],
                });
            }
        } else {
            payload_chunk_counts.push(0);
        }
    }

    let (proof_ixs, proof_pdas) = chunking::chunk_proof(&chunk_ctx, proof_bytes)?;
    let proof_chunk_count = u8::try_from(proof_ixs.len())
        .map_err(|_| eyre::eyre!("proof chunk count exceeds u8::MAX"))?;
    for pda in &proof_pdas {
        chunk_account_metas.push(solana_sdk::instruction::AccountMeta::new(*pda, false));
    }
    for ix in proof_ixs {
        chunk_messages.push(SolanaMessage {
            instructions: vec![ix],
        });
    }

    let proof = mercury_solana::ibc_types::MsgProof {
        height: proof_height,
        data: mercury_solana::ibc_types::Delivery::Chunked {
            total_chunks: proof_chunk_count,
        },
    };

    let packet_message = build_packet_ix(msg_packet, proof, chunk_account_metas.clone())?;

    let has_chunks = !chunk_messages.is_empty();
    let cleanup_message = if has_chunks {
        let cleanup_ix =
            chunking::cleanup_chunks(&chunk_ctx, &payload_chunk_counts, proof_chunk_count)?;
        wrap_cleanup_message(vec![cleanup_ix])
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

        let mut msg_packet = cosmos_packet_to_msg_packet(packet);
        let proof_bytes = &proof.proof_bytes;
        let height_val = proof_height.value();

        let output = build_chunked_packet_message(
            ChunkedPacketParams {
                ics26_program_id: &chain.ics26_program_id,
                access_manager_program_id: &access_mgr,
                payer: &payer,
                client_id: dest_client_id,
                sequence,
                msg_packet: &mut msg_packet,
                proof_bytes,
                proof_height: height_val,
            },
            |pkt, proof_meta, chunk_metas| {
                let msg = mercury_solana::ibc_types::MsgRecvPacket {
                    packet: pkt.clone(),
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
                let ix = instructions::recv_packet(&params, &msg, chunk_metas)?;
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

        let mut msg_packet = cosmos_packet_to_msg_packet(packet);
        let proof_bytes = &proof.proof_bytes;
        let height_val = proof_height.value();
        let ack_bytes = ack.0.clone();

        let output = build_chunked_packet_message(
            ChunkedPacketParams {
                ics26_program_id: &chain.ics26_program_id,
                access_manager_program_id: &access_mgr,
                payer: &payer,
                client_id: source_client_id,
                sequence,
                msg_packet: &mut msg_packet,
                proof_bytes,
                proof_height: height_val,
            },
            |pkt, proof_meta, chunk_metas| {
                let msg = mercury_solana::ibc_types::MsgAckPacket {
                    packet: pkt.clone(),
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
                let ix = instructions::ack_packet(&params, &msg, chunk_metas)?;
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

        let mut msg_packet = packet.to_msg_packet();
        let proof_bytes = &proof.proof_bytes;
        let height_val = proof_height.value();

        let output = build_chunked_packet_message(
            ChunkedPacketParams {
                ics26_program_id: &chain.ics26_program_id,
                access_manager_program_id: &access_mgr,
                payer: &payer,
                client_id: source_client_id,
                sequence,
                msg_packet: &mut msg_packet,
                proof_bytes,
                proof_height: height_val,
            },
            |pkt, proof_meta, chunk_metas| {
                let msg = mercury_solana::ibc_types::MsgTimeoutPacket {
                    packet: pkt.clone(),
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
                let ix = instructions::timeout_packet(&params, &msg, chunk_metas)?;
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

        let timestamp_nanos = tm_consensus_state.timestamp.unix_timestamp_nanos();
        let timestamp = u64::try_from(timestamp_nanos)
            .map_err(|_| eyre::eyre!("consensus state has negative or overflowing timestamp"))?;

        let consensus_state = mercury_solana::ibc_types::ConsensusState {
            timestamp,
            root: root_bytes,
            next_validators_hash: next_val_hash,
        };

        let client_id = crate::DEFAULT_TENDERMINT_CLIENT_ID;

        let counterparty_client_id = payload.counterparty_client_id.ok_or_else(|| {
            eyre::eyre!(
                "CosmosCreateClientPayload.counterparty_client_id is required \
                 when creating a client on Solana"
            )
        })?;

        let counterparty_merkle_prefix = payload
            .counterparty_merkle_prefix
            .unwrap_or_else(mercury_core::MerklePrefix::ibc_default);

        let counterparty_info = mercury_solana::ibc_types::CounterpartyInfo {
            client_id: counterparty_client_id,
            merkle_prefix: counterparty_merkle_prefix.0,
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

        let borsh_header = mercury_solana::borsh_header::header_to_borsh(header);
        let header_bytes =
            borsh::to_vec(&borsh_header).map_err(|e| eyre::eyre!("borsh serialize header: {e}"))?;

        let mut messages = Vec::new();

        let header_chunks = chunking::chunk_data(&header_bytes);
        let mut header_chunk_pdas = Vec::with_capacity(header_chunks.len());
        for (i, chunk) in header_chunks.into_iter().enumerate() {
            let chunk_idx = u8::try_from(i)
                .map_err(|_| eyre::eyre!("header chunk index {i} exceeds u8::MAX"))?;
            let ix = signatures::upload_header_chunk(
                &ics07,
                &payer,
                target_height,
                chunk_idx,
                chunk,
                &access_mgr,
            )?;
            messages.push(SolanaMessage {
                instructions: vec![ix],
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

        let mut cleanup_pdas = header_chunk_pdas;
        cleanup_pdas.extend_from_slice(&sig_verify_pdas);
        if !cleanup_pdas.is_empty() {
            let cleanup_ix = signatures::cleanup_incomplete_upload(&ics07, &payer, &cleanup_pdas)?;
            messages.push(SolanaMessage {
                instructions: mercury_solana::instructions::with_compute_budget(cleanup_ix),
            });
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

#[cfg(test)]
mod tests {
    use mercury_cosmos::builders::CosmosCreateClientPayload;

    #[test]
    fn payload_without_counterparty_client_id_is_none_by_default() {
        let p = CosmosCreateClientPayload::default();
        assert!(p.counterparty_client_id.is_none());
    }
}
