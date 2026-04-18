use borsh::BorshSerialize;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use crate::accounts::{self, AccessManager, Ics26Router};

pub const CHUNK_DATA_SIZE: usize = 900;

pub struct ChunkContext<'a> {
    pub ics26_program_id: &'a Pubkey,
    pub payer: &'a Pubkey,
    pub client_id: &'a str,
    pub sequence: u64,
    pub access_manager_program_id: &'a Pubkey,
}

#[must_use]
pub fn chunk_data(data: &[u8]) -> Vec<Vec<u8>> {
    data.chunks(CHUNK_DATA_SIZE).map(<[u8]>::to_vec).collect()
}

#[must_use]
pub const fn needs_chunking(data: &[u8]) -> bool {
    data.len() > CHUNK_DATA_SIZE
}

#[derive(BorshSerialize)]
struct UploadPayloadChunkArgs {
    client_id: String,
    sequence: u64,
    payload_index: u8,
    chunk_index: u8,
    chunk_data: Vec<u8>,
}

fn upload_payload_chunk(
    ctx: &ChunkContext<'_>,
    payload_index: u8,
    chunk_index: u8,
    chunk_data: Vec<u8>,
) -> eyre::Result<Instruction> {
    let args = UploadPayloadChunkArgs {
        client_id: ctx.client_id.to_string(),
        sequence: ctx.sequence,
        payload_index,
        chunk_index,
        chunk_data,
    };
    let data = accounts::encode_anchor_instruction("upload_payload_chunk", &args)?;

    let (router_state, _) = Ics26Router::router_state_pda(ctx.ics26_program_id);
    let (access_manager, _) = AccessManager::pda(ctx.access_manager_program_id);
    let (payload_chunk, _) = Ics26Router::payload_chunk_pda(
        ctx.payer,
        ctx.client_id,
        ctx.sequence,
        payload_index,
        chunk_index,
        ctx.ics26_program_id,
    );

    Ok(Instruction {
        program_id: *ctx.ics26_program_id,
        accounts: vec![
            AccountMeta::new_readonly(router_state, false),
            AccountMeta::new_readonly(access_manager, false),
            AccountMeta::new(payload_chunk, false),
            AccountMeta::new(*ctx.payer, true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            AccountMeta::new_readonly(solana_sdk::sysvar::instructions::ID, false),
        ],
        data,
    })
}

#[derive(BorshSerialize)]
struct UploadProofChunkArgs {
    client_id: String,
    sequence: u64,
    payload_index: u8,
    chunk_index: u8,
    chunk_data: Vec<u8>,
}

fn upload_proof_chunk(
    ctx: &ChunkContext<'_>,
    chunk_index: u8,
    chunk_data: Vec<u8>,
) -> eyre::Result<Instruction> {
    let args = UploadProofChunkArgs {
        client_id: ctx.client_id.to_string(),
        sequence: ctx.sequence,
        payload_index: 0,
        chunk_index,
        chunk_data,
    };
    let data = accounts::encode_anchor_instruction("upload_proof_chunk", &args)?;

    let (router_state, _) = Ics26Router::router_state_pda(ctx.ics26_program_id);
    let (access_manager, _) = AccessManager::pda(ctx.access_manager_program_id);
    let (proof_chunk, _) = Ics26Router::proof_chunk_pda(
        ctx.payer,
        ctx.client_id,
        ctx.sequence,
        chunk_index,
        ctx.ics26_program_id,
    );

    Ok(Instruction {
        program_id: *ctx.ics26_program_id,
        accounts: vec![
            AccountMeta::new_readonly(router_state, false),
            AccountMeta::new_readonly(access_manager, false),
            AccountMeta::new(proof_chunk, false),
            AccountMeta::new(*ctx.payer, true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            AccountMeta::new_readonly(solana_sdk::sysvar::instructions::ID, false),
        ],
        data,
    })
}

pub fn chunk_payload(
    ctx: &ChunkContext<'_>,
    payload_index: u8,
    payload_data: &[u8],
) -> eyre::Result<(Vec<Instruction>, Vec<Pubkey>)> {
    let chunks = chunk_data(payload_data);
    let mut instructions = Vec::with_capacity(chunks.len());
    let mut pdas = Vec::with_capacity(chunks.len());

    for (i, chunk) in chunks.into_iter().enumerate() {
        let chunk_index =
            u8::try_from(i).map_err(|_| eyre::eyre!("chunk index {i} exceeds u8::MAX"))?;
        let (pda, _) = Ics26Router::payload_chunk_pda(
            ctx.payer,
            ctx.client_id,
            ctx.sequence,
            payload_index,
            chunk_index,
            ctx.ics26_program_id,
        );
        pdas.push(pda);
        instructions.push(upload_payload_chunk(
            ctx,
            payload_index,
            chunk_index,
            chunk,
        )?);
    }

    Ok((instructions, pdas))
}

pub fn chunk_proof(
    ctx: &ChunkContext<'_>,
    proof_data: &[u8],
) -> eyre::Result<(Vec<Instruction>, Vec<Pubkey>)> {
    let chunks = chunk_data(proof_data);
    let mut instructions = Vec::with_capacity(chunks.len());
    let mut pdas = Vec::with_capacity(chunks.len());

    for (i, chunk) in chunks.into_iter().enumerate() {
        let chunk_index =
            u8::try_from(i).map_err(|_| eyre::eyre!("chunk index {i} exceeds u8::MAX"))?;
        let (pda, _) = Ics26Router::proof_chunk_pda(
            ctx.payer,
            ctx.client_id,
            ctx.sequence,
            chunk_index,
            ctx.ics26_program_id,
        );
        pdas.push(pda);
        instructions.push(upload_proof_chunk(ctx, chunk_index, chunk)?);
    }

    Ok((instructions, pdas))
}

pub fn cleanup_chunks(
    ctx: &ChunkContext<'_>,
    payload_chunk_counts: &[u8],
    proof_chunk_count: u8,
) -> eyre::Result<Instruction> {
    #[derive(BorshSerialize)]
    struct CleanupChunksArgs {
        client_id: String,
        sequence: u64,
        payload_chunks: Vec<u8>,
        total_proof_chunks: u8,
    }

    let args = CleanupChunksArgs {
        client_id: ctx.client_id.to_string(),
        sequence: ctx.sequence,
        payload_chunks: payload_chunk_counts.to_vec(),
        total_proof_chunks: proof_chunk_count,
    };
    let data = accounts::encode_anchor_instruction("cleanup_chunks", &args)?;

    let (router_state, _) = Ics26Router::router_state_pda(ctx.ics26_program_id);
    let (access_manager, _) = AccessManager::pda(ctx.access_manager_program_id);

    let mut account_metas = vec![
        AccountMeta::new_readonly(router_state, false),
        AccountMeta::new_readonly(access_manager, false),
        AccountMeta::new(*ctx.payer, true),
        AccountMeta::new_readonly(solana_sdk::sysvar::instructions::ID, false),
    ];

    for (payload_idx, &chunks) in payload_chunk_counts.iter().enumerate() {
        let p_idx = u8::try_from(payload_idx)
            .map_err(|_| eyre::eyre!("payload index {payload_idx} exceeds u8::MAX"))?;
        for chunk_idx in 0..chunks {
            let (pda, _) = Ics26Router::payload_chunk_pda(
                ctx.payer,
                ctx.client_id,
                ctx.sequence,
                p_idx,
                chunk_idx,
                ctx.ics26_program_id,
            );
            account_metas.push(AccountMeta::new(pda, false));
        }
    }

    for chunk_idx in 0..proof_chunk_count {
        let (pda, _) = Ics26Router::proof_chunk_pda(
            ctx.payer,
            ctx.client_id,
            ctx.sequence,
            chunk_idx,
            ctx.ics26_program_id,
        );
        account_metas.push(AccountMeta::new(pda, false));
    }

    Ok(Instruction {
        program_id: *ctx.ics26_program_id,
        accounts: account_metas,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_data_single() {
        let data = vec![0u8; 500];
        let chunks = chunk_data(&data);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 500);
    }

    #[test]
    fn chunk_data_multiple() {
        let data = vec![0u8; 2000];
        let chunks = chunk_data(&data);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), CHUNK_DATA_SIZE);
        assert_eq!(chunks[1].len(), CHUNK_DATA_SIZE);
        assert_eq!(chunks[2].len(), 2000 - 2 * CHUNK_DATA_SIZE);
    }

    #[test]
    fn needs_chunking_boundary() {
        assert!(!needs_chunking(&[0u8; CHUNK_DATA_SIZE]));
        assert!(needs_chunking(&[0u8; CHUNK_DATA_SIZE + 1]));
    }
}
