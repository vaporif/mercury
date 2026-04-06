use borsh::BorshSerialize;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use crate::accounts::{self, Ics26Router};

pub const CHUNK_DATA_SIZE: usize = 900;
pub const HEADER_CHUNK_DATA_SIZE: usize = 750;

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

pub fn upload_payload_chunk(
    ics26_program_id: &Pubkey,
    payer: &Pubkey,
    client_id: &str,
    sequence: u64,
    payload_index: u8,
    chunk_index: u8,
    chunk_data: Vec<u8>,
) -> eyre::Result<Instruction> {
    let args = UploadPayloadChunkArgs {
        client_id: client_id.to_string(),
        sequence,
        payload_index,
        chunk_index,
        chunk_data,
    };
    let data = accounts::encode_anchor_instruction("upload_payload_chunk", &args)?;

    let (payload_chunk, _) = Ics26Router::payload_chunk_pda(
        payer,
        client_id,
        sequence,
        payload_index,
        chunk_index,
        ics26_program_id,
    );

    Ok(Instruction {
        program_id: *ics26_program_id,
        accounts: vec![
            AccountMeta::new(payload_chunk, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
        ],
        data,
    })
}

#[derive(BorshSerialize)]
struct UploadProofChunkArgs {
    client_id: String,
    sequence: u64,
    chunk_index: u8,
    chunk_data: Vec<u8>,
}

pub fn upload_proof_chunk(
    ics26_program_id: &Pubkey,
    payer: &Pubkey,
    client_id: &str,
    sequence: u64,
    chunk_index: u8,
    chunk_data: Vec<u8>,
) -> eyre::Result<Instruction> {
    let args = UploadProofChunkArgs {
        client_id: client_id.to_string(),
        sequence,
        chunk_index,
        chunk_data,
    };
    let data = accounts::encode_anchor_instruction("upload_proof_chunk", &args)?;

    let (proof_chunk, _) =
        Ics26Router::proof_chunk_pda(payer, client_id, sequence, chunk_index, ics26_program_id);

    Ok(Instruction {
        program_id: *ics26_program_id,
        accounts: vec![
            AccountMeta::new(proof_chunk, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
        ],
        data,
    })
}

pub fn chunk_payload(
    ics26_program_id: &Pubkey,
    payer: &Pubkey,
    client_id: &str,
    sequence: u64,
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
            payer,
            client_id,
            sequence,
            payload_index,
            chunk_index,
            ics26_program_id,
        );
        pdas.push(pda);
        instructions.push(upload_payload_chunk(
            ics26_program_id,
            payer,
            client_id,
            sequence,
            payload_index,
            chunk_index,
            chunk,
        )?);
    }

    Ok((instructions, pdas))
}

pub fn chunk_proof(
    ics26_program_id: &Pubkey,
    payer: &Pubkey,
    client_id: &str,
    sequence: u64,
    proof_data: &[u8],
) -> eyre::Result<(Vec<Instruction>, Vec<Pubkey>)> {
    let chunks = chunk_data(proof_data);
    let mut instructions = Vec::with_capacity(chunks.len());
    let mut pdas = Vec::with_capacity(chunks.len());

    for (i, chunk) in chunks.into_iter().enumerate() {
        let chunk_index =
            u8::try_from(i).map_err(|_| eyre::eyre!("chunk index {i} exceeds u8::MAX"))?;
        let (pda, _) =
            Ics26Router::proof_chunk_pda(payer, client_id, sequence, chunk_index, ics26_program_id);
        pdas.push(pda);
        instructions.push(upload_proof_chunk(
            ics26_program_id,
            payer,
            client_id,
            sequence,
            chunk_index,
            chunk,
        )?);
    }

    Ok((instructions, pdas))
}

pub fn cleanup_payload_chunks(
    ics26_program_id: &Pubkey,
    payer: &Pubkey,
    client_id: &str,
    sequence: u64,
    payload_count: u8,
    chunk_counts: &[u8],
) -> eyre::Result<Vec<Instruction>> {
    #[derive(BorshSerialize)]
    struct CleanupPayloadChunksArgs {
        client_id: String,
        sequence: u64,
    }

    let args = CleanupPayloadChunksArgs {
        client_id: client_id.to_string(),
        sequence,
    };
    let data = accounts::encode_anchor_instruction("cleanup_payload_chunks", &args)?;

    let mut account_metas = vec![
        AccountMeta::new(*payer, true),
        AccountMeta::new_readonly(solana_system_interface::program::ID, false),
    ];

    for payload_idx in 0..payload_count {
        let chunks = chunk_counts.get(payload_idx as usize).copied().unwrap_or(0);
        for chunk_idx in 0..chunks {
            let (pda, _) = Ics26Router::payload_chunk_pda(
                payer,
                client_id,
                sequence,
                payload_idx,
                chunk_idx,
                ics26_program_id,
            );
            account_metas.push(AccountMeta::new(pda, false));
        }
    }

    Ok(vec![Instruction {
        program_id: *ics26_program_id,
        accounts: account_metas,
        data,
    }])
}

pub fn cleanup_proof_chunks(
    ics26_program_id: &Pubkey,
    payer: &Pubkey,
    client_id: &str,
    sequence: u64,
    chunk_count: u8,
) -> eyre::Result<Vec<Instruction>> {
    #[derive(BorshSerialize)]
    struct CleanupProofChunksArgs {
        client_id: String,
        sequence: u64,
    }

    let args = CleanupProofChunksArgs {
        client_id: client_id.to_string(),
        sequence,
    };
    let data = accounts::encode_anchor_instruction("cleanup_proof_chunks", &args)?;

    let mut account_metas = vec![
        AccountMeta::new(*payer, true),
        AccountMeta::new_readonly(solana_system_interface::program::ID, false),
    ];

    for chunk_idx in 0..chunk_count {
        let (pda, _) =
            Ics26Router::proof_chunk_pda(payer, client_id, sequence, chunk_idx, ics26_program_id);
        account_metas.push(AccountMeta::new(pda, false));
    }

    Ok(vec![Instruction {
        program_id: *ics26_program_id,
        accounts: account_metas,
        data,
    }])
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
