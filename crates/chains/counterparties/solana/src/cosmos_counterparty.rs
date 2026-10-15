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

/// Cosmos emits the proto-encoded `Acknowledgement`; the Solana router hashes
/// the single app acknowledgement itself, so unwrap it.
fn single_app_ack(encoded: &[u8]) -> Result<Vec<u8>> {
    use prost::Message as _;
    let ack = ibc_proto::ibc::core::channel::v2::Acknowledgement::decode(encoded)
        .map_err(|e| eyre::eyre!("decoding Cosmos acknowledgement: {e}"))?;
    let [app_ack] = <[Vec<u8>; 1]>::try_from(ack.app_acknowledgements).map_err(|acks| {
        eyre::eyre!(
            "Solana supports single-payload acknowledgements, got {}",
            acks.len()
        )
    })?;
    Ok(app_ack)
}

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
        let ack_bytes = single_app_ack(&ack.0)?;

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
        let selected_sigs = signatures::select_minimal_signatures(&header, &all_signatures)?;

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

    #[expect(clippy::expect_used, reason = "trait method is infallible")]
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
    type CounterpartyUpdateHeader = mercury_cosmos::misbehaviour::OnChainTmConsensusState;

    async fn query_consensus_state_heights(
        &self,
        _client_id: &SolanaClientId,
    ) -> Result<Vec<TmHeight>> {
        use solana_client::rpc_filter::{Memcmp, MemcmpEncodedBytes, RpcFilterType};

        let ics07_program_id = self.0.ics07_program_id.ok_or_else(|| {
            eyre::eyre!("ics07_program_id not configured — required for misbehaviour scan")
        })?;

        let discriminator = accounts::anchor_account_discriminator(
            <ConsensusStateStore as accounts::AnchorAccount>::NAME,
        );

        let filter_accounts = self
            .0
            .rpc
            .get_program_accounts_with_filter(
                &ics07_program_id,
                vec![RpcFilterType::Memcmp(Memcmp::new(
                    0,
                    MemcmpEncodedBytes::Bytes(discriminator.to_vec()),
                ))],
            )
            .await?;

        let mut heights = Vec::new();
        for (_pubkey, account) in &filter_accounts {
            let Ok(store) =
                accounts::deserialize_anchor_account::<ConsensusStateStore>(&account.data)
            else {
                continue;
            };
            if let Ok(h) = TmHeight::try_from(store.height) {
                heights.push(h);
            }
        }

        heights.sort_unstable_by(|a, b| b.cmp(a));

        tracing::debug!(
            count = heights.len(),
            "found Solana consensus state heights"
        );
        Ok(heights)
    }

    async fn query_update_client_header(
        &self,
        _client_id: &SolanaClientId,
        consensus_height: &TmHeight,
    ) -> Result<Option<mercury_cosmos::misbehaviour::OnChainTmConsensusState>> {
        let ics07_program_id = self
            .0
            .ics07_program_id
            .ok_or_else(|| eyre::eyre!("ics07_program_id not configured"))?;

        let height = consensus_height.value();
        let (pda, _bump) =
            accounts::Ics07Tendermint::consensus_state_pda(height, &ics07_program_id);

        let store: Option<ConsensusStateStore> = accounts::fetch_account(&self.0.rpc, &pda).await?;

        Ok(store.and_then(ConsensusStateStore::into_on_chain))
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

#[derive(borsh::BorshDeserialize)]
struct ConsensusStateStore {
    height: u64,
    state: accounts::OnChainConsensusState,
}

impl accounts::AnchorAccount for ConsensusStateStore {
    const NAME: &'static str = "ConsensusStateStore";
}

impl ConsensusStateStore {
    fn into_on_chain(self) -> Option<mercury_cosmos::misbehaviour::OnChainTmConsensusState> {
        let height = TmHeight::try_from(self.height).ok()?;
        Some(mercury_cosmos::misbehaviour::OnChainTmConsensusState {
            height,
            timestamp_nanos: u128::from(self.state.timestamp),
            root: self.state.root,
            next_validators_hash: self.state.next_validators_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mercury_cosmos::builders::CosmosCreateClientPayload;
    use mercury_cosmos::misbehaviour::OnChainTmConsensusState;

    fn encoded_ack(app_acks: Vec<Vec<u8>>) -> Vec<u8> {
        use prost::Message as _;
        ibc_proto::ibc::core::channel::v2::Acknowledgement {
            app_acknowledgements: app_acks,
        }
        .encode_to_vec()
    }

    #[test]
    fn single_app_ack_unwraps_proto() {
        let ack = single_app_ack(&encoded_ack(vec![b"ok".to_vec()])).ok();
        assert_eq!(ack, Some(b"ok".to_vec()));
    }

    #[test]
    fn single_app_ack_rejects_multi_payload() {
        assert!(single_app_ack(&encoded_ack(vec![b"a".to_vec(), b"b".to_vec()])).is_err());
        assert!(single_app_ack(&encoded_ack(vec![])).is_err());
    }

    #[test]
    fn payload_without_counterparty_client_id_is_none_by_default() {
        let p = CosmosCreateClientPayload::default();
        assert!(p.counterparty_client_id.is_none());
    }

    #[test]
    fn deserialize_consensus_state_store() {
        let mut data = Vec::new();
        let discriminator = accounts::anchor_account_discriminator("ConsensusStateStore");
        data.extend_from_slice(&discriminator);
        data.extend_from_slice(&42u64.to_le_bytes());
        data.extend_from_slice(&1_000_000_000u64.to_le_bytes());
        data.extend_from_slice(&[0xAA; 32]);
        data.extend_from_slice(&[0xBB; 32]);

        let store = accounts::deserialize_anchor_account::<ConsensusStateStore>(&data).unwrap();
        let cs = store.into_on_chain().unwrap();

        assert_eq!(cs.height, TmHeight::try_from(42u64).unwrap());
        assert_eq!(cs.timestamp_nanos, 1_000_000_000u128);
        assert_eq!(cs.root, [0xAA; 32]);
        assert_eq!(cs.next_validators_hash, [0xBB; 32]);
    }

    #[test]
    fn deserialize_consensus_state_store_short_data() {
        let data = vec![0u8; 20];
        assert!(accounts::deserialize_anchor_account::<ConsensusStateStore>(&data).is_err());
    }

    #[test]
    fn wrap_cleanup_message_empty_returns_none() {
        assert!(wrap_cleanup_message(vec![]).is_none());
    }

    #[test]
    fn wrap_cleanup_message_single_ix_wraps_with_budget() {
        let ix = solana_sdk::instruction::Instruction::new_with_bytes(
            solana_sdk::pubkey::Pubkey::new_unique(),
            &[1, 2, 3],
            vec![],
        );
        let msg = wrap_cleanup_message(vec![ix]).unwrap();
        // with_compute_budget adds 2 budget instructions before the actual ix
        assert_eq!(msg.instructions.len(), 3);
    }

    #[test]
    fn wrap_cleanup_message_multiple_ixs() {
        let make_ix = || {
            solana_sdk::instruction::Instruction::new_with_bytes(
                solana_sdk::pubkey::Pubkey::new_unique(),
                &[],
                vec![],
            )
        };
        let msg = wrap_cleanup_message(vec![make_ix(), make_ix()]).unwrap();
        // 2 budget + first ix + second ix
        assert_eq!(msg.instructions.len(), 4);
    }

    #[test]
    fn flatten_chunked_output_no_chunks() {
        let packet_msg = SolanaMessage {
            instructions: vec![solana_sdk::instruction::Instruction::new_with_bytes(
                solana_sdk::pubkey::Pubkey::new_unique(),
                &[],
                vec![],
            )],
        };
        let output = ChunkedPacketOutput {
            chunk_messages: vec![],
            packet_message: packet_msg,
            cleanup_message: None,
        };
        let result = flatten_chunked_output(output);
        assert_eq!(result.instructions.len(), 1);
    }

    #[test]
    fn flatten_chunked_output_with_chunks_and_cleanup() {
        let make_msg = |n: usize| SolanaMessage {
            instructions: (0..n)
                .map(|_| {
                    solana_sdk::instruction::Instruction::new_with_bytes(
                        solana_sdk::pubkey::Pubkey::new_unique(),
                        &[],
                        vec![],
                    )
                })
                .collect(),
        };
        let output = ChunkedPacketOutput {
            chunk_messages: vec![make_msg(2), make_msg(1)],
            packet_message: make_msg(1),
            cleanup_message: Some(make_msg(3)),
        };
        let result = flatten_chunked_output(output);
        // 2 + 1 (chunks) + 1 (packet) + 3 (cleanup) = 7
        assert_eq!(result.instructions.len(), 7);
    }

    #[test]
    fn cosmos_packet_to_msg_packet_roundtrip() {
        use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};
        use mercury_cosmos::types::{PacketPayload, RawClientId};

        let packet = CosmosPacket {
            source_client_id: RawClientId("src-client".into()),
            dest_client_id: RawClientId("dst-client".into()),
            sequence: PacketSequence(42),
            timeout_timestamp: TimeoutTimestamp(1_000_000),
            payloads: vec![PacketPayload {
                source_port: Port("transfer".into()),
                dest_port: Port("transfer".into()),
                version: "v1".into(),
                encoding: "proto3".into(),
                data: vec![0xDE, 0xAD],
            }],
        };

        let msg = cosmos_packet_to_msg_packet(&packet);
        assert_eq!(msg.sequence, 42);
        assert_eq!(msg.source_client, "src-client");
        assert_eq!(msg.dest_client, "dst-client");
        assert_eq!(msg.timeout_timestamp, 1_000_000);
        assert_eq!(msg.payloads.len(), 1);
        assert_eq!(msg.payloads[0].source_port, "transfer");
        assert_eq!(msg.payloads[0].dest_port, "transfer");
        assert_eq!(msg.payloads[0].version, "v1");
        assert_eq!(msg.payloads[0].encoding, "proto3");
    }

    #[test]
    fn consensus_state_matches_ignores_height() {
        let a = OnChainTmConsensusState {
            height: TmHeight::try_from(1u64).unwrap(),
            timestamp_nanos: 1000,
            root: [0xAA; 32],
            next_validators_hash: [0xBB; 32],
        };
        let b = OnChainTmConsensusState {
            height: TmHeight::try_from(99u64).unwrap(),
            ..a
        };
        assert!(a.matches_fields(&b));
    }
}

#[cfg(test)]
mod builder_tests {
    use super::*;

    use crate::test_support::{PORT, adapter, deployed, tm_header};
    use borsh::BorshDeserialize;
    use ibc::core::client::types::Height as IbcHeight;
    use ibc::core::commitment_types::commitment::CommitmentRoot;
    use ibc::core::commitment_types::specs::ProofSpecs;
    use ibc::core::host::types::identifiers::ChainId;
    use ibc_client_tendermint::types::{
        AllowUpdate, ClientState as TmClientState, ConsensusState as TmConsensusState,
        Header as TmIbcHeader, TrustThreshold,
    };
    use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};
    use mercury_cosmos::keys::Secp256k1KeyPair;
    use mercury_cosmos::types::{PacketPayload, RawClientId};
    use mercury_solana::accounts::{
        ANCHOR_DISCRIMINATOR_LEN, AccessManager, IbcApp, Ics07Tendermint,
        anchor_instruction_discriminator,
    };
    use mercury_solana::ibc_types::{
        ClientState as SolClientState, ConsensusState as SolConsensusState, CounterpartyInfo,
        Delivery, MsgAckPacket, MsgRecvPacket, MsgTimeoutPacket,
    };
    use mercury_solana::rpc::testing::anchor_account;
    use mercury_solana::types::SolanaPayload;
    use solana_sdk::instruction::{AccountMeta, Instruction};
    use solana_sdk::pubkey::Pubkey;

    type Cosmos = CosmosChain<Secp256k1KeyPair>;

    const DEST_CLIENT: &str = "07-tendermint-0";
    const SOURCE_CLIENT: &str = "08-wasm-0";
    const SEQ: u64 = 7;
    const PROOF_HEIGHT: u64 = 42;
    fn payload(len: usize) -> PacketPayload {
        PacketPayload {
            source_port: Port(PORT.into()),
            dest_port: Port(PORT.into()),
            version: "ics20-1".into(),
            encoding: "application/json".into(),
            data: (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect(),
        }
    }

    fn cosmos_packet(payloads: Vec<PacketPayload>) -> CosmosPacket {
        CosmosPacket {
            source_client_id: RawClientId(SOURCE_CLIENT.into()),
            dest_client_id: RawClientId(DEST_CLIENT.into()),
            sequence: PacketSequence(SEQ),
            timeout_timestamp: TimeoutTimestamp(1_000),
            payloads,
        }
    }

    fn solana_packet(data_len: usize) -> SolanaPacket {
        SolanaPacket {
            source_client_id: DEST_CLIENT.into(),
            dest_client_id: SOURCE_CLIENT.into(),
            sequence: PacketSequence(SEQ),
            timeout_timestamp: TimeoutTimestamp(1_000),
            payloads: vec![SolanaPayload {
                source_port: Port(PORT.into()),
                dest_port: Port(PORT.into()),
                version: "ics20-1".into(),
                encoding: "application/json".into(),
                data: vec![5; data_len],
            }],
        }
    }

    fn proof(len: usize) -> MerkleProof {
        MerkleProof {
            proof_bytes: (0..len).map(|i| u8::try_from(i % 13).unwrap()).collect(),
        }
    }

    fn height(h: u64) -> TmHeight {
        TmHeight::try_from(h).unwrap()
    }

    fn decode<T: BorshDeserialize>(ix: &Instruction, method: &str) -> T {
        let (disc, body) = ix.data.split_at(ANCHOR_DISCRIMINATOR_LEN);
        assert_eq!(disc, anchor_instruction_discriminator(method), "{method}");
        T::try_from_slice(body).unwrap()
    }

    fn is_method(ix: &Instruction, method: &str) -> bool {
        ix.data.get(..ANCHOR_DISCRIMINATOR_LEN) == Some(&anchor_instruction_discriminator(method))
    }

    fn find<'a>(msg: &'a SolanaMessage, method: &str) -> Vec<&'a Instruction> {
        msg.instructions
            .iter()
            .filter(|ix| is_method(ix, method))
            .collect()
    }

    fn payer(a: &SolanaAdapter) -> Pubkey {
        a.0.keypair.pubkey()
    }

    // Mirrors ics26-router `UploadPayloadChunk` / `UploadProofChunk` args.
    type ChunkArgs = (String, u64, u8, u8, Vec<u8>);
    // Mirrors ics26-router `MsgCleanupChunks`.
    type CleanupArgs = (String, u64, Vec<u8>, u8);

    // The router reassembles chunks in index order (utils/chunking.rs), so
    // concatenating the uploads must give back the original bytes.
    fn reassemble(msg: &SolanaMessage, method: &str, payload_index: u8) -> Vec<u8> {
        let mut chunks: Vec<ChunkArgs> = find(msg, method)
            .into_iter()
            .map(|ix| decode::<ChunkArgs>(ix, method))
            .filter(|c| c.2 == payload_index)
            .collect();
        chunks.sort_by_key(|c| c.3);
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(usize::from(c.3), i, "chunk indices must be contiguous");
        }
        chunks.into_iter().flat_map(|c| c.4).collect()
    }

    // Remaining accounts after the fixed `RecvPacket`/`AckPacket` accounts.
    fn remaining(ix: &Instruction, fixed: usize) -> Vec<Pubkey> {
        ix.accounts[fixed..].iter().map(|m| m.pubkey).collect()
    }

    const RECV_FIXED_ACCOUNTS: usize = 14;
    const ACK_FIXED_ACCOUNTS: usize = 13;
    const CLEANUP_FIXED_ACCOUNTS: usize = 4;

    // --- packet messages ---

    #[tokio::test]
    async fn recv_small_payload_stays_inline_but_proof_is_always_chunked() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let a = adapter(&d, rpc);
        let pkt = cosmos_packet(vec![payload(10)]);
        let msg = PacketMessageBuilder::<Cosmos>::build_receive_packet_message(
            &a,
            &pkt,
            proof(100),
            height(PROOF_HEIGHT),
            0,
        )
        .await
        .unwrap();

        // proof upload, then [budget, budget, recv], then [budget, budget, cleanup]
        assert_eq!(msg.instructions.len(), 7);
        let recv = find(&msg, "recv_packet");
        assert_eq!(recv.len(), 1);
        let decoded: MsgRecvPacket = decode(recv[0], "recv_packet");
        assert_eq!(decoded.packet.sequence, SEQ);
        assert_eq!(decoded.packet.source_client, SOURCE_CLIENT);
        assert_eq!(decoded.packet.dest_client, DEST_CLIENT);
        assert!(
            matches!(&decoded.packet.payloads[0].data, Delivery::Inline { data } if *data == pkt.payloads[0].data)
        );
        assert_eq!(decoded.proof.height, PROOF_HEIGHT);
        assert!(matches!(
            decoded.proof.data,
            Delivery::Chunked { total_chunks: 1 }
        ));

        let proof_pda = Ics26Router::proof_chunk_pda(&payer(&a), DEST_CLIENT, SEQ, 0, &d.ics26).0;
        assert_eq!(remaining(recv[0], RECV_FIXED_ACCOUNTS), vec![proof_pda]);
        assert_eq!(
            reassemble(&msg, "upload_proof_chunk", 0),
            proof(100).proof_bytes
        );

        // Light client accounts are resolved through the dest client's ics07 program.
        let keys: Vec<_> = recv[0].accounts.iter().map(|m| m.pubkey).collect();
        assert!(keys.contains(&d.ics07));
        assert!(keys.contains(&Ics07Tendermint::consensus_state_pda(PROOF_HEIGHT, &d.ics07).0));
        assert!(keys.contains(&AccessManager::pda(&d.am).0));
        assert!(keys.contains(&IbcApp::state_pda(&d.app).0));

        let cleanup = find(&msg, "cleanup_chunks");
        let args: CleanupArgs = decode(cleanup[0], "cleanup_chunks");
        assert_eq!(args, (DEST_CLIENT.to_string(), SEQ, vec![0], 1));
        assert_eq!(
            remaining(cleanup[0], CLEANUP_FIXED_ACCOUNTS),
            vec![proof_pda]
        );
    }

    #[tokio::test]
    async fn payloads_switch_to_chunked_delivery_at_300_bytes_total() {
        for (len, chunked) in [(299, false), (300, true)] {
            let (d, rpc) = deployed(DEST_CLIENT);
            let a = adapter(&d, rpc);
            let msg = PacketMessageBuilder::<Cosmos>::build_receive_packet_message(
                &a,
                &cosmos_packet(vec![payload(len)]),
                proof(10),
                height(PROOF_HEIGHT),
                0,
            )
            .await
            .unwrap();
            let decoded: MsgRecvPacket = decode(find(&msg, "recv_packet")[0], "recv_packet");
            assert_eq!(
                matches!(
                    decoded.packet.payloads[0].data,
                    Delivery::Chunked { total_chunks: 1 }
                ),
                chunked,
                "payload of {len} bytes"
            );
            assert_eq!(
                find(&msg, "upload_payload_chunk").len(),
                usize::from(chunked)
            );
        }
    }

    #[tokio::test]
    async fn chunked_payloads_and_proof_follow_router_remaining_account_order() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let a = adapter(&d, rpc);
        // 200 B fits one chunk, 1000 B needs two, the 1801 B proof needs three.
        let pkt = cosmos_packet(vec![payload(200), payload(1000)]);
        let msg = PacketMessageBuilder::<Cosmos>::build_receive_packet_message(
            &a,
            &pkt,
            proof(1801),
            height(PROOF_HEIGHT),
            0,
        )
        .await
        .unwrap();

        let decoded: MsgRecvPacket = decode(find(&msg, "recv_packet")[0], "recv_packet");
        // The router rejects mixed inline/chunked payloads, so both must be chunked.
        let counts: Vec<u8> = decoded
            .packet
            .payloads
            .iter()
            .map(|p| match p.data {
                Delivery::Chunked { total_chunks } => total_chunks,
                Delivery::Inline { .. } => panic!("mixed delivery modes"),
            })
            .collect();
        assert_eq!(counts, vec![1, 2]);
        assert!(matches!(
            decoded.proof.data,
            Delivery::Chunked { total_chunks: 3 }
        ));

        let p = payer(&a);
        let payload_pda =
            |pi, ci| Ics26Router::payload_chunk_pda(&p, DEST_CLIENT, SEQ, pi, ci, &d.ics26).0;
        let proof_pda = |ci| Ics26Router::proof_chunk_pda(&p, DEST_CLIENT, SEQ, ci, &d.ics26).0;
        // Router: payload chunks (per payload, in order), then proof chunks.
        let expected = vec![
            payload_pda(0, 0),
            payload_pda(1, 0),
            payload_pda(1, 1),
            proof_pda(0),
            proof_pda(1),
            proof_pda(2),
        ];
        let recv = find(&msg, "recv_packet")[0];
        assert_eq!(remaining(recv, RECV_FIXED_ACCOUNTS), expected);
        assert!(
            recv.accounts[RECV_FIXED_ACCOUNTS..]
                .iter()
                .all(|m| m.is_writable && !m.is_signer)
        );

        assert_eq!(
            reassemble(&msg, "upload_payload_chunk", 0),
            pkt.payloads[0].data
        );
        assert_eq!(
            reassemble(&msg, "upload_payload_chunk", 1),
            pkt.payloads[1].data
        );
        assert_eq!(
            reassemble(&msg, "upload_proof_chunk", 0),
            proof(1801).proof_bytes
        );

        let cleanup = find(&msg, "cleanup_chunks")[0];
        let args: CleanupArgs = decode(cleanup, "cleanup_chunks");
        assert_eq!(args, (DEST_CLIENT.to_string(), SEQ, vec![1, 2], 3));
        assert_eq!(remaining(cleanup, CLEANUP_FIXED_ACCOUNTS), expected);

        // Uploads come first so the chunks exist when the packet is delivered.
        let recv_pos = msg
            .instructions
            .iter()
            .position(|ix| is_method(ix, "recv_packet"))
            .unwrap();
        let cleanup_pos = msg
            .instructions
            .iter()
            .position(|ix| is_method(ix, "cleanup_chunks"))
            .unwrap();
        assert_eq!(recv_pos, 6 + 2, "six uploads, then the compute budget");
        assert!(cleanup_pos > recv_pos);
    }

    use solana_packet::PACKET_DATA_SIZE;

    async fn sent_tx_sizes(payload_len: usize, proof_len: usize) -> Vec<usize> {
        let (d, rpc) = deployed(DEST_CLIENT);
        let sent = rpc.sent_txs.clone();
        let a = adapter(&d, rpc);
        let msg = PacketMessageBuilder::<Cosmos>::build_receive_packet_message(
            &a,
            &cosmos_packet(vec![payload(payload_len)]),
            proof(proof_len),
            height(PROOF_HEIGHT),
            0,
        )
        .await
        .unwrap();
        mercury_chain_traits::types::MessageSender::send_messages(&a.0, vec![msg])
            .await
            .unwrap();
        sent.lock().unwrap().iter().map(Vec::len).collect()
    }

    #[tokio::test]
    async fn single_chunk_recv_goes_out_as_three_packet_sized_transactions() {
        // proof upload, recv, cleanup
        let sizes = sent_tx_sizes(10, 100).await;
        assert_eq!(sizes.len(), 3);
        assert!(sizes.iter().all(|&s| s <= PACKET_DATA_SIZE), "{sizes:?}");
    }

    // Chunk uploads carry no compute-budget prefix, so `send_messages` (which
    // splits only at SetComputeUnitLimit) packs every upload into one
    // transaction: a 1801-byte proof yields a single ~2.3 KB transaction.
    #[tokio::test]
    #[ignore = "bug: consecutive chunk uploads are sent as one oversized transaction"]
    async fn each_chunk_upload_is_sent_as_its_own_transaction() {
        // three proof uploads, recv, cleanup
        let sizes = sent_tx_sizes(10, 1801).await;
        assert_eq!(sizes.len(), 5, "{sizes:?}");
        // payload upload, proof upload, recv, cleanup
        let sizes = sent_tx_sizes(400, 600).await;
        assert_eq!(sizes.len(), 4, "{sizes:?}");
        assert!(sizes.iter().all(|&s| s <= PACKET_DATA_SIZE), "{sizes:?}");
    }

    // Two chunks carry 2 * CHUNK_DATA_SIZE = 1800 data bytes, so they cannot
    // share a transaction whatever the account encoding (ALT or not).
    #[tokio::test]
    #[ignore = "bug: two chunk uploads share one transaction larger than PACKET_DATA_SIZE"]
    async fn chunked_recv_transactions_fit_in_a_packet() {
        let two_proof_chunks = sent_tx_sizes(10, 1801).await;
        let payload_and_proof_chunk = sent_tx_sizes(400, 600).await;
        assert!(
            two_proof_chunks
                .iter()
                .chain(&payload_and_proof_chunk)
                .all(|&s| s <= PACKET_DATA_SIZE),
            "limit {PACKET_DATA_SIZE}: two proof chunks {two_proof_chunks:?}, \
             payload+proof chunk {payload_and_proof_chunk:?}"
        );
    }

    #[tokio::test]
    async fn ack_unwraps_single_app_ack_and_targets_source_client() {
        let (d, rpc) = deployed(SOURCE_CLIENT);
        let a = adapter(&d, rpc);
        // Ack is relayed back to the packet's source; flip the cosmos packet ids.
        let mut pkt = cosmos_packet(vec![payload(10)]);
        std::mem::swap(&mut pkt.source_client_id, &mut pkt.dest_client_id);
        pkt.source_client_id = RawClientId(SOURCE_CLIENT.into());
        let ack = PacketAcknowledgement(encoded_ack(vec![b"app-ack".to_vec()]));

        let msg = PacketMessageBuilder::<Cosmos>::build_ack_packet_message(
            &a,
            &pkt,
            &ack,
            proof(50),
            height(PROOF_HEIGHT),
            0,
        )
        .await
        .unwrap();

        let ix = find(&msg, "ack_packet")[0];
        let decoded: MsgAckPacket = decode(ix, "ack_packet");
        assert_eq!(decoded.acknowledgement, b"app-ack".to_vec());
        assert_eq!(decoded.proof.height, PROOF_HEIGHT);
        assert!(ix.accounts.contains(&AccountMeta::new(
            Ics26Router::packet_commitment_pda(SOURCE_CLIENT, SEQ, &d.ics26).0,
            false
        )));
        let proof_pda = Ics26Router::proof_chunk_pda(&payer(&a), SOURCE_CLIENT, SEQ, 0, &d.ics26).0;
        assert_eq!(remaining(ix, ACK_FIXED_ACCOUNTS), vec![proof_pda]);

        let multi = PacketAcknowledgement(encoded_ack(vec![b"a".to_vec(), b"b".to_vec()]));
        let err = PacketMessageBuilder::<Cosmos>::build_ack_packet_message(
            &a,
            &pkt,
            &multi,
            proof(50),
            height(PROOF_HEIGHT),
            0,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("single-payload"), "{err}");

        let garbage = PacketAcknowledgement(vec![0xFF; 3]);
        let err = PacketMessageBuilder::<Cosmos>::build_ack_packet_message(
            &a,
            &pkt,
            &garbage,
            proof(50),
            height(PROOF_HEIGHT),
            0,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("decoding Cosmos acknowledgement"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn timeout_chunks_proof_under_solana_source_client() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let a = adapter(&d, rpc);
        let pkt = solana_packet(10);
        let msg = PacketMessageBuilder::<Cosmos>::build_timeout_packet_message(
            &a,
            &pkt,
            proof(64),
            height(PROOF_HEIGHT),
            0,
        )
        .await
        .unwrap();

        let ix = find(&msg, "timeout_packet")[0];
        let decoded: MsgTimeoutPacket = decode(ix, "timeout_packet");
        assert_eq!(decoded.packet.source_client, DEST_CLIENT);
        assert_eq!(decoded.packet.dest_client, SOURCE_CLIENT);
        assert!(ix.accounts.contains(&AccountMeta::new(
            Ics26Router::packet_commitment_pda(DEST_CLIENT, SEQ, &d.ics26).0,
            false
        )));
        assert_eq!(
            reassemble(&msg, "upload_proof_chunk", 0),
            proof(64).proof_bytes
        );
        let args: CleanupArgs = decode(find(&msg, "cleanup_chunks")[0], "cleanup_chunks");
        assert_eq!(args, (DEST_CLIENT.to_string(), SEQ, vec![0], 1));
    }

    #[tokio::test]
    async fn packet_builders_reject_empty_payloads() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let a = adapter(&d, rpc);
        let empty = cosmos_packet(vec![]);
        let mut empty_solana = solana_packet(1);
        empty_solana.payloads.clear();
        let ack = PacketAcknowledgement(encoded_ack(vec![b"ok".to_vec()]));

        let errs = [
            PacketMessageBuilder::<Cosmos>::build_receive_packet_message(
                &a,
                &empty,
                proof(1),
                height(1),
                0,
            )
            .await
            .unwrap_err(),
            PacketMessageBuilder::<Cosmos>::build_ack_packet_message(
                &a,
                &empty,
                &ack,
                proof(1),
                height(1),
                0,
            )
            .await
            .unwrap_err(),
            PacketMessageBuilder::<Cosmos>::build_timeout_packet_message(
                &a,
                &empty_solana,
                proof(1),
                height(1),
                0,
            )
            .await
            .unwrap_err(),
        ];
        for err in errs {
            assert!(err.to_string().contains("packet has no payloads"), "{err}");
        }
    }

    #[tokio::test]
    async fn packet_builders_surface_missing_on_chain_accounts() {
        let pkt = cosmos_packet(vec![payload(1)]);
        let build = |a: SolanaAdapter| {
            let pkt = pkt.clone();
            async move {
                PacketMessageBuilder::<Cosmos>::build_receive_packet_message(
                    &a,
                    &pkt,
                    proof(1),
                    height(1),
                    0,
                )
                .await
                .unwrap_err()
                .to_string()
            }
        };

        let (d, mut rpc) = deployed(DEST_CLIENT);
        rpc.accounts
            .remove(&Ics26Router::client_pda(DEST_CLIENT, &d.ics26).0);
        assert!(
            build(adapter(&d, rpc))
                .await
                .contains("client PDA not found")
        );

        let (d, mut rpc) = deployed(DEST_CLIENT);
        rpc.accounts
            .remove(&Ics26Router::router_state_pda(&d.ics26).0);
        assert!(
            build(adapter(&d, rpc))
                .await
                .contains("router state PDA not found")
        );

        let (d, mut rpc) = deployed(DEST_CLIENT);
        rpc.accounts
            .remove(&Ics26Router::ibc_app_pda(PORT, &d.ics26).0);
        assert!(
            build(adapter(&d, rpc))
                .await
                .contains("IbcApp PDA not found")
        );
    }

    fn encoded_ack(app_acks: Vec<Vec<u8>>) -> Vec<u8> {
        use prost::Message as _;
        ibc_proto::ibc::core::channel::v2::Acknowledgement {
            app_acknowledgements: app_acks,
        }
        .encode_to_vec()
    }

    // --- create client ---

    fn tm_client_state(latest: u64) -> TmClientState {
        TmClientState::new(
            ChainId::new("cosmoshub-4").unwrap(),
            TrustThreshold::new(2, 3).unwrap(),
            Duration::from_hours(336),
            Duration::from_hours(504),
            Duration::from_secs(15),
            IbcHeight::new(4, latest).unwrap(),
            ProofSpecs::cosmos(),
            vec!["upgrade".into()],
            AllowUpdate {
                after_expiry: false,
                after_misbehaviour: false,
            },
        )
        .unwrap()
    }

    fn tm_consensus_state(root: &[u8], unix_secs: i64) -> TmConsensusState {
        TmConsensusState::new(
            CommitmentRoot::from_bytes(root),
            tendermint::Time::from_unix_timestamp(unix_secs, 5).unwrap(),
            tendermint::Hash::Sha256([0xBB; 32]),
        )
    }

    fn create_payload(root: &[u8], unix_secs: i64) -> CosmosCreateClientPayload {
        CosmosCreateClientPayload {
            client_state: tm_client_state(100).into(),
            consensus_state: tm_consensus_state(root, unix_secs).into(),
            counterparty_client_id: Some(SOURCE_CLIENT.into()),
            counterparty_merkle_prefix: None,
        }
    }

    type AddClientArgs = (String, CounterpartyInfo);
    type InitializeArgs = (SolClientState, SolConsensusState, Pubkey);

    #[tokio::test]
    async fn create_client_registers_then_initializes_ics07() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let a = adapter(&d, rpc);
        let msg = ClientMessageBuilder::<Cosmos>::build_create_client_message(
            &a,
            create_payload(&[0xAA; 32], 1_700_000_000),
        )
        .await
        .unwrap();

        // [budget, budget, add_client, budget, budget, initialize]
        assert_eq!(msg.instructions.len(), 6);
        let add = &msg.instructions[2];
        assert_eq!(add.program_id, d.ics26);
        let (client_id, info): AddClientArgs = decode(add, "add_client");
        assert_eq!(client_id, crate::DEFAULT_TENDERMINT_CLIENT_ID);
        assert_eq!(info.client_id, SOURCE_CLIENT);
        assert_eq!(
            info.merkle_prefix,
            mercury_core::MerklePrefix::ibc_default().0
        );

        let init = &msg.instructions[5];
        assert_eq!(init.program_id, d.ics07);
        let (cs, cons, am): InitializeArgs = decode(init, "initialize");
        assert_eq!(cs.chain_id, "cosmoshub-4");
        assert_eq!(
            (cs.trust_level_numerator, cs.trust_level_denominator),
            (2, 3)
        );
        assert_eq!(cs.trusting_period, 1_209_600);
        assert_eq!(cs.unbonding_period, 1_814_400);
        assert_eq!(cs.max_clock_drift, 15);
        assert_eq!(
            (
                cs.latest_height.revision_number,
                cs.latest_height.revision_height
            ),
            (4, 100)
        );
        assert_eq!(
            cs.frozen_height.revision_height, 0,
            "new client is not frozen"
        );
        // ics07 ConsensusState.timestamp is unix nanoseconds.
        assert_eq!(cons.timestamp, 1_700_000_000_000_000_005);
        assert_eq!(cons.root, [0xAA; 32]);
        assert_eq!(cons.next_validators_hash, [0xBB; 32]);
        assert_eq!(am, d.am);
        // initialize stores the consensus state at the client's latest height.
        assert_eq!(
            init.accounts[1].pubkey,
            Ics07Tendermint::consensus_state_pda(100, &d.ics07).0
        );
    }

    #[tokio::test]
    async fn create_client_uses_explicit_merkle_prefix() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let a = adapter(&d, rpc);
        let mut p = create_payload(&[0xAA; 32], 1);
        let prefix = mercury_core::MerklePrefix(vec![b"store".to_vec()]);
        p.counterparty_merkle_prefix = Some(prefix.clone());
        let msg = ClientMessageBuilder::<Cosmos>::build_create_client_message(&a, p)
            .await
            .unwrap();
        let (_, info): AddClientArgs = decode(&msg.instructions[2], "add_client");
        assert_eq!(info.merkle_prefix, prefix.0);
    }

    #[tokio::test]
    async fn create_client_rejects_invalid_inputs() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let a = adapter(&d, rpc);
        let err = |p| async {
            ClientMessageBuilder::<Cosmos>::build_create_client_message(&a, p)
                .await
                .unwrap_err()
                .to_string()
        };

        let mut p = create_payload(&[0xAA; 32], 1);
        p.counterparty_client_id = None;
        assert!(err(p).await.contains("counterparty_client_id is required"));

        let mut p = create_payload(&[0xAA; 32], 1);
        p.client_state = tm_consensus_state(&[0; 32], 1).into();
        assert!(err(p).await.contains("failed to decode client state"));

        let mut p = create_payload(&[0xAA; 32], 1);
        p.consensus_state = tm_client_state(1).into();
        assert!(err(p).await.contains("failed to decode consensus state"));

        let p = create_payload(&[0xAA; 20], 1);
        assert!(err(p).await.contains("root is not 32 bytes"));

        // ibc-rs already rejects pre-epoch timestamps while decoding, ahead of
        // the builder's own u64 guard.
        let p = create_payload(&[0xAA; 32], -10);
        assert!(err(p).await.contains("failed to decode consensus state"));

        let mut no_ics07 = a.clone();
        no_ics07.0.ics07_program_id = None;
        let e = ClientMessageBuilder::<Cosmos>::build_create_client_message(
            &no_ics07,
            create_payload(&[0xAA; 32], 1),
        )
        .await
        .unwrap_err();
        assert!(
            e.to_string().contains("ics07_program_id not configured"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn register_counterparty_is_folded_into_add_client() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let a = adapter(&d, rpc);
        let err = ClientMessageBuilder::<Cosmos>::build_register_counterparty_message(
            &a,
            &SolanaClientId(DEST_CLIENT.into()),
            &"07-tendermint-1".parse().unwrap(),
            mercury_core::MerklePrefix::ibc_default(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("add_client"), "{err}");
    }

    // --- update client ---

    fn update_payload(headers: Vec<TmIbcHeader>) -> CosmosUpdateClientPayload {
        CosmosUpdateClientPayload {
            headers: headers.into_iter().map(Into::into).collect(),
            trusted_consensus_state: None,
            membership_proofs: mercury_core::MembershipProofs::default(),
        }
    }

    // Mirrors ics07-tendermint `upload_header_chunk(target_height, chunk_index, chunk_data)`.
    type HeaderChunkArgs = (u64, u8, Vec<u8>);

    #[tokio::test]
    async fn update_client_uploads_borsh_header_and_assembles_inline_when_below_threshold() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let a = adapter(&d, rpc);
        let header = tm_header(&[10, 10, 10, 10], &[0, 1, 2, 3], 120, 100);
        let expected_bytes = borsh::to_vec(&mercury_solana::borsh_header::header_to_borsh(
            header.clone(),
        ))
        .unwrap();

        let out = ClientMessageBuilder::<Cosmos>::build_update_client_message(
            &a,
            &SolanaClientId(DEST_CLIENT.into()),
            update_payload(vec![header]),
        )
        .await
        .unwrap();
        assert!(out.membership_proof.is_none());

        let n_chunks = expected_bytes.len().div_ceil(chunking::CHUNK_DATA_SIZE);
        // one message per header chunk, then assemble, then cleanup
        assert_eq!(out.messages.len(), n_chunks + 2);
        let p = payer(&a);
        let mut uploaded = Vec::new();
        for (i, m) in out.messages[..n_chunks].iter().enumerate() {
            let ix = &m.instructions[0];
            assert_eq!(ix.program_id, d.ics07);
            let (target, idx, data): HeaderChunkArgs = decode(ix, "upload_header_chunk");
            assert_eq!((target, usize::from(idx)), (120, i));
            uploaded.extend(data);
        }
        assert_eq!(uploaded, expected_bytes);

        let chunk_pdas: Vec<_> = (0..n_chunks)
            .map(|i| {
                Ics07Tendermint::header_chunk_pda(&p, 120, u8::try_from(i).unwrap(), &d.ics07).0
            })
            .collect();
        // No signature pre-verification below the threshold.
        assert_eq!(
            find(&out.messages[n_chunks], "pre_verify_signature"),
            Vec::<&Instruction>::new()
        );
        let cleanup = &out.messages[n_chunks + 1];
        assert_eq!(cleanup.instructions.len(), 3, "compute budget + cleanup");
        let cleanup_keys: Vec<_> = cleanup.instructions[2]
            .accounts
            .iter()
            .map(|m| m.pubkey)
            .collect();
        for pda in &chunk_pdas {
            assert!(cleanup_keys.contains(pda));
        }
    }

    #[tokio::test]
    async fn update_client_pre_verifies_minimal_signature_set_above_threshold() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let mut a = adapter(&d, rpc);
        a.0.config.skip_pre_verify_threshold = Some(2);
        // Total power 100, threshold 67: the two heaviest signers (50 + 30) suffice.
        let header = tm_header(&[50, 30, 15, 5], &[3, 2, 1, 0], 120, 100);

        let out = ClientMessageBuilder::<Cosmos>::build_update_client_message(
            &a,
            &SolanaClientId(DEST_CLIENT.into()),
            update_payload(vec![header]),
        )
        .await
        .unwrap();

        let pre_verify: Vec<_> = out
            .messages
            .iter()
            .filter(|m| {
                m.instructions
                    .iter()
                    .any(|ix| is_method(ix, "pre_verify_signature"))
            })
            .collect();
        assert_eq!(pre_verify.len(), 2);
        // Every pre-verify tx carries the ed25519 precompile check it relies on.
        for m in &pre_verify {
            assert!(
                m.instructions
                    .iter()
                    .any(|ix| ix.program_id == solana_sdk::ed25519_program::ID)
            );
        }
        // ics07 keys sig-verify PDAs by sha256(signature).
        let sig_pda = |byte: u8| {
            let hash = solana_sdk::hash::hash(&[byte; 64]).to_bytes();
            Ics07Tendermint::sig_verify_pda(&hash, &d.ics07).0
        };
        // Validator 0 (power 50) signed with byte 10, validator 1 (power 30) with 11.
        let cleanup = out.messages.last().unwrap();
        let keys: Vec<_> = cleanup.instructions[2]
            .accounts
            .iter()
            .map(|m| m.pubkey)
            .collect();
        assert!(keys.contains(&sig_pda(10)));
        assert!(keys.contains(&sig_pda(11)));
        assert!(!keys.contains(&sig_pda(12)));
    }

    #[tokio::test]
    async fn update_client_rejects_bad_payloads() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let a = adapter(&d, rpc);
        let id = SolanaClientId(DEST_CLIENT.into());
        let run = |p| async {
            match ClientMessageBuilder::<Cosmos>::build_update_client_message(&a, &id, p).await {
                Ok(_) => panic!("expected an error"),
                Err(e) => e.to_string(),
            }
        };

        assert!(run(update_payload(vec![])).await.contains("no headers"));

        let mut not_header = update_payload(vec![]);
        not_header.headers.push(tm_client_state(1).into());
        assert!(
            run(not_header)
                .await
                .contains("failed to decode tendermint header")
        );

        // 10 of 30 voting power cannot reach the 2/3 quorum.
        let weak = tm_header(&[10, 10, 10], &[0], 120, 100);
        assert!(
            run(update_payload(vec![weak]))
                .await
                .contains("voting power")
        );
    }

    // --- queries ---

    type WireClientState = (String, u64, u64, u64, u64, u64, (u64, u64), (u64, u64));

    fn client_state_account(trusting_period: u64, latest: u64) -> Vec<u8> {
        let body: WireClientState = (
            "cosmoshub-4".into(),
            2,
            3,
            trusting_period,
            2 * trusting_period,
            10,
            (0, 0),
            (4, latest),
        );
        anchor_account("ClientState", &body)
    }

    fn consensus_store(height: u64, timestamp: u64) -> Vec<u8> {
        anchor_account(
            "ConsensusStateStore",
            &(height, (timestamp, [0xAA_u8; 32], [0xBB_u8; 32])),
        )
    }

    #[tokio::test]
    async fn client_queries_resolve_through_ics07_program() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let rpc = rpc
            .with_account(
                Ics07Tendermint::client_state_pda(&d.ics07).0,
                client_state_account(3_600, 55),
            )
            .with_account(
                Ics07Tendermint::consensus_state_pda(55, &d.ics07).0,
                consensus_store(55, 9),
            );
        let a = adapter(&d, rpc);
        let id = SolanaClientId(DEST_CLIENT.into());

        let cs = ClientQuery::<Cosmos>::query_client_state(&a, &id, &SolanaHeight(0))
            .await
            .unwrap();
        assert_eq!(
            <SolanaAdapter as ClientQuery<Cosmos>>::client_latest_height(&cs),
            height(55)
        );
        assert_eq!(
            <SolanaAdapter as ClientQuery<Cosmos>>::trusting_period(&cs),
            Some(Duration::from_secs(3_600))
        );

        let cons =
            ClientQuery::<Cosmos>::query_consensus_state(&a, &id, &height(55), &SolanaHeight(0))
                .await
                .unwrap();
        assert_eq!(cons.0, consensus_store(55, 9));
        assert!(
            ClientQuery::<Cosmos>::query_consensus_state(&a, &id, &height(56), &SolanaHeight(0))
                .await
                .is_err()
        );
    }

    #[test]
    #[should_panic(expected = "height conversion")]
    fn client_latest_height_panics_beyond_tendermint_height_range() {
        // Tendermint heights are capped at i64::MAX.
        let cs = SolanaClientState(client_state_account(1, u64::MAX));
        let _ = <SolanaAdapter as ClientQuery<Cosmos>>::client_latest_height(&cs);
    }

    #[tokio::test]
    async fn consensus_heights_scan_filters_by_discriminator_and_sorts_descending() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let mut truncated = consensus_store(77, 1);
        truncated.truncate(20);
        let rpc = rpc
            .with_program_account(d.ics07, Pubkey::new_unique(), consensus_store(5, 1))
            .with_program_account(d.ics07, Pubkey::new_unique(), consensus_store(20, 1))
            .with_program_account(d.ics07, Pubkey::new_unique(), consensus_store(10, 1))
            // Beyond the Tendermint height range (i64::MAX).
            .with_program_account(d.ics07, Pubkey::new_unique(), consensus_store(u64::MAX, 1))
            .with_program_account(d.ics07, Pubkey::new_unique(), truncated)
            // Different account type owned by the same program.
            .with_program_account(d.ics07, Pubkey::new_unique(), client_state_account(1, 99))
            // Same type, different program.
            .with_program_account(
                Pubkey::new_unique(),
                Pubkey::new_unique(),
                consensus_store(30, 1),
            );
        let a = adapter(&d, rpc);

        let heights = MisbehaviourQuery::<Cosmos>::query_consensus_state_heights(
            &a,
            &SolanaClientId(DEST_CLIENT.into()),
        )
        .await
        .unwrap();
        assert_eq!(heights, vec![height(20), height(10), height(5)]);
    }

    #[tokio::test]
    async fn update_client_header_reads_consensus_store_pda() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let rpc = rpc
            .with_account(
                Ics07Tendermint::consensus_state_pda(12, &d.ics07).0,
                consensus_store(12, 1_000),
            )
            .with_account(
                Ics07Tendermint::consensus_state_pda(13, &d.ics07).0,
                // Stored height is outside the Tendermint height range.
                consensus_store(u64::MAX, 1_000),
            );
        let a = adapter(&d, rpc);
        let id = SolanaClientId(DEST_CLIENT.into());

        let found = MisbehaviourQuery::<Cosmos>::query_update_client_header(&a, &id, &height(12))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.height, height(12));
        assert_eq!(found.timestamp_nanos, 1_000);
        assert_eq!(found.root, [0xAA; 32]);
        assert_eq!(found.next_validators_hash, [0xBB; 32]);

        assert!(
            MisbehaviourQuery::<Cosmos>::query_update_client_header(&a, &id, &height(13))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            MisbehaviourQuery::<Cosmos>::query_update_client_header(&a, &id, &height(14))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn misbehaviour_paths_need_ics07_program_and_are_not_implemented() {
        let (d, rpc) = deployed(DEST_CLIENT);
        let mut a = adapter(&d, rpc);
        a.0.ics07_program_id = None;
        let id = SolanaClientId(DEST_CLIENT.into());

        let err = MisbehaviourQuery::<Cosmos>::query_consensus_state_heights(&a, &id)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("ics07_program_id not configured"),
            "{err}"
        );
        let err = MisbehaviourQuery::<Cosmos>::query_update_client_header(&a, &id, &height(1))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("ics07_program_id not configured"),
            "{err}"
        );

        let header = tm_header(&[1], &[0], 2, 1);
        let cs = mercury_cosmos::client_types::CosmosClientState::Tendermint(tm_client_state(1));
        let none = MisbehaviourDetector::<Cosmos>::check_for_misbehaviour(
            &a,
            &"07-tendermint-0".parse().unwrap(),
            &header,
            &cs,
        )
        .await
        .unwrap();
        assert!(none.is_none());

        let evidence = mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence::CorrectiveUpdate {
            payload: update_payload(vec![]),
        };
        assert!(
            MisbehaviourMessageBuilder::<Cosmos>::build_misbehaviour_message(&a, &id, evidence)
                .await
                .is_err()
        );
    }
}
