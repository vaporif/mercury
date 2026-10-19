use borsh::BorshSerialize;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use crate::accounts::{self, AccessManager, Ics26Router};

/// Largest chunk the on-chain programs accept.
pub const CHUNK_DATA_SIZE: usize = 900;

/// Payload/proof chunk size the relayer uploads.
///
/// A full `CHUNK_DATA_SIZE` upload with its compute budget prefix and a 32-byte client id is ~1350
/// bytes, over the 1232-byte transaction limit unless an ALT shortens it.
pub const PACKET_CHUNK_SIZE: usize = 768;

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

fn chunk_packet_data(data: &[u8]) -> Vec<Vec<u8>> {
    data.chunks(PACKET_CHUNK_SIZE).map(<[u8]>::to_vec).collect()
}

pub fn chunk_payload(
    ctx: &ChunkContext<'_>,
    payload_index: u8,
    payload_data: &[u8],
) -> eyre::Result<(Vec<Instruction>, Vec<Pubkey>)> {
    let chunks = chunk_packet_data(payload_data);
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
    let chunks = chunk_packet_data(proof_data);
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

    use crate::accounts::{ANCHOR_DISCRIMINATOR_LEN, anchor_instruction_discriminator};

    type WireUploadArgs = (String, u64, u8, u8, Vec<u8>);
    type WireCleanupArgs = (String, u64, Vec<u8>, u8);

    const MAX_CHUNKS: usize = u8::MAX as usize + 1;

    struct Keys {
        ics26: Pubkey,
        payer: Pubkey,
        am: Pubkey,
    }

    impl Keys {
        fn new() -> Self {
            Self {
                ics26: Pubkey::new_unique(),
                payer: Pubkey::new_unique(),
                am: Pubkey::new_unique(),
            }
        }

        fn ctx(&self) -> ChunkContext<'_> {
            ChunkContext {
                ics26_program_id: &self.ics26,
                payer: &self.payer,
                client_id: "07-tendermint-0",
                sequence: 9,
                access_manager_program_id: &self.am,
            }
        }
    }

    fn decode<T: borsh::BorshDeserialize>(ix: &Instruction, method: &str) -> T {
        let (disc, body) = ix.data.split_at(ANCHOR_DISCRIMINATOR_LEN);
        assert_eq!(disc, anchor_instruction_discriminator(method));
        borsh::from_slice(body).unwrap()
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
    }

    #[test]
    fn chunk_data_edges() {
        assert_eq!(chunk_data(&[]), Vec::<Vec<u8>>::new());

        let exact = chunk_data(&[0u8; 2 * CHUNK_DATA_SIZE]);
        assert_eq!(exact.len(), 2);
        assert!(exact.iter().all(|c| c.len() == CHUNK_DATA_SIZE));

        let over = chunk_data(&[0u8; 2 * CHUNK_DATA_SIZE + 1]);
        assert_eq!(over.len(), 3);
        assert_eq!(over[2].len(), 1);
    }

    #[test]
    fn chunk_payload_round_trips_data_and_derives_matching_pdas() {
        let k = Keys::new();
        let data = pattern(2 * CHUNK_DATA_SIZE + 17);
        let (ixs, pdas) = chunk_payload(&k.ctx(), 3, &data).unwrap();
        assert_eq!(ixs.len(), 3);
        assert_eq!(pdas.len(), 3);

        let mut reassembled = Vec::new();
        for (i, (ix, pda)) in ixs.iter().zip(&pdas).enumerate() {
            let (client_id, seq, payload_idx, chunk_idx, chunk): WireUploadArgs =
                decode(ix, "upload_payload_chunk");
            assert_eq!(client_id, "07-tendermint-0");
            assert_eq!(seq, 9);
            assert_eq!(payload_idx, 3);
            assert_eq!(usize::from(chunk_idx), i);
            reassembled.extend(chunk);

            let expected_pda = Ics26Router::payload_chunk_pda(
                &k.payer,
                "07-tendermint-0",
                9,
                3,
                chunk_idx,
                &k.ics26,
            )
            .0;
            assert_eq!(*pda, expected_pda);
            assert_eq!(ix.program_id, k.ics26);
            assert_eq!(
                ix.accounts,
                vec![
                    AccountMeta::new_readonly(Ics26Router::router_state_pda(&k.ics26).0, false),
                    AccountMeta::new_readonly(AccessManager::pda(&k.am).0, false),
                    AccountMeta::new(expected_pda, false),
                    AccountMeta::new(k.payer, true),
                    AccountMeta::new_readonly(solana_system_interface::program::ID, false),
                    AccountMeta::new_readonly(solana_sdk::sysvar::instructions::ID, false),
                ]
            );
        }
        assert_eq!(reassembled, data);
    }

    #[test]
    fn chunk_proof_uses_proof_pdas_and_zero_payload_index() {
        let k = Keys::new();
        let data = pattern(CHUNK_DATA_SIZE + 1);
        let (ixs, pdas) = chunk_proof(&k.ctx(), &data).unwrap();
        assert_eq!(ixs.len(), 2);

        let mut reassembled = Vec::new();
        for (i, (ix, pda)) in ixs.iter().zip(&pdas).enumerate() {
            let (_, seq, payload_idx, chunk_idx, chunk): WireUploadArgs =
                decode(ix, "upload_proof_chunk");
            assert_eq!(seq, 9);
            assert_eq!(payload_idx, 0);
            assert_eq!(usize::from(chunk_idx), i);
            reassembled.extend(chunk);
            let expected =
                Ics26Router::proof_chunk_pda(&k.payer, "07-tendermint-0", 9, chunk_idx, &k.ics26).0;
            assert_eq!(*pda, expected);
            assert_eq!(ix.accounts[2], AccountMeta::new(expected, false));
            assert_eq!(ix.accounts[3], AccountMeta::new(k.payer, true));
        }
        assert_eq!(reassembled, data);
    }

    #[test]
    fn chunking_empty_input_produces_nothing() {
        let k = Keys::new();
        let (ixs, pdas) = chunk_payload(&k.ctx(), 0, &[]).unwrap();
        assert!(ixs.is_empty() && pdas.is_empty());
        let (ixs, pdas) = chunk_proof(&k.ctx(), &[]).unwrap();
        assert!(ixs.is_empty() && pdas.is_empty());
    }

    #[test]
    fn chunk_index_limit_is_u8() {
        let k = Keys::new();
        let max = vec![0u8; MAX_CHUNKS * PACKET_CHUNK_SIZE];
        let (ixs, _) = chunk_payload(&k.ctx(), 0, &max).unwrap();
        assert_eq!(ixs.len(), MAX_CHUNKS);

        let over = vec![0u8; MAX_CHUNKS * PACKET_CHUNK_SIZE + 1];
        let err = chunk_payload(&k.ctx(), 0, &over).unwrap_err();
        assert!(err.to_string().contains("exceeds u8::MAX"), "{err}");
        assert!(chunk_proof(&k.ctx(), &over).is_err());
    }

    #[test]
    fn cleanup_chunks_lists_every_payload_then_proof_pda() {
        let k = Keys::new();
        let ix = cleanup_chunks(&k.ctx(), &[2, 0, 1], 2).unwrap();

        let (client_id, seq, payload_chunks, proof_chunks): WireCleanupArgs =
            decode(&ix, "cleanup_chunks");
        assert_eq!(client_id, "07-tendermint-0");
        assert_eq!(seq, 9);
        assert_eq!(payload_chunks, vec![2, 0, 1]);
        assert_eq!(proof_chunks, 2);

        let payload = |p, c| {
            AccountMeta::new(
                Ics26Router::payload_chunk_pda(&k.payer, "07-tendermint-0", 9, p, c, &k.ics26).0,
                false,
            )
        };
        let proof = |c| {
            AccountMeta::new(
                Ics26Router::proof_chunk_pda(&k.payer, "07-tendermint-0", 9, c, &k.ics26).0,
                false,
            )
        };
        assert_eq!(
            ix.accounts,
            vec![
                AccountMeta::new_readonly(Ics26Router::router_state_pda(&k.ics26).0, false),
                AccountMeta::new_readonly(AccessManager::pda(&k.am).0, false),
                AccountMeta::new(k.payer, true),
                AccountMeta::new_readonly(solana_sdk::sysvar::instructions::ID, false),
                payload(0, 0),
                payload(0, 1),
                payload(2, 0),
                proof(0),
                proof(1),
            ]
        );
    }

    #[test]
    fn cleanup_chunks_with_nothing_to_close_has_only_fixed_accounts() {
        let k = Keys::new();
        let ix = cleanup_chunks(&k.ctx(), &[], 0).unwrap();
        assert_eq!(ix.accounts.len(), 4);
    }

    #[test]
    fn cleanup_chunks_rejects_more_than_256_payloads() {
        let k = Keys::new();
        assert!(cleanup_chunks(&k.ctx(), &[0u8; MAX_CHUNKS], 0).is_ok());
        let err = cleanup_chunks(&k.ctx(), &[0u8; MAX_CHUNKS + 1], 0).unwrap_err();
        assert!(err.to_string().contains("payload index 256"), "{err}");
    }
}
