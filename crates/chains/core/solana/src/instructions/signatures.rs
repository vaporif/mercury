use borsh::BorshSerialize;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::sysvar;

use super::{COMPUTE_UNIT_LIMIT, COMPUTE_UNIT_PRICE};
use crate::accounts::{self, Ics07Tendermint};

#[derive(BorshSerialize)]
struct UploadHeaderChunkArgs {
    target_height: u64,
    chunk_index: u8,
    chunk_data: Vec<u8>,
}

#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn upload_header_chunk(
    ics07_program_id: &Pubkey,
    payer: &Pubkey,
    target_height: u64,
    chunk_index: u8,
    chunk_data: Vec<u8>,
) -> Instruction {
    let args = UploadHeaderChunkArgs {
        target_height,
        chunk_index,
        chunk_data,
    };
    let data = accounts::encode_anchor_instruction("upload_header_chunk", &args);

    let (header_chunk_pda, _) =
        Ics07Tendermint::header_chunk_pda(payer, target_height, chunk_index, ics07_program_id);

    Instruction {
        program_id: *ics07_program_id,
        accounts: vec![
            AccountMeta::new(header_chunk_pda, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
        ],
        data,
    }
}

#[must_use]
pub fn assemble_and_update_client(
    ics07_program_id: &Pubkey,
    payer: &Pubkey,
    old_consensus_height: u64,
    new_consensus_height: u64,
    header_chunk_pdas: &[Pubkey],
    sig_verify_pdas: &[Pubkey],
) -> Vec<Instruction> {
    let disc = accounts::anchor_instruction_discriminator("assemble_and_update_client");
    let data = disc.to_vec();

    let (client_state, _) = Ics07Tendermint::client_state_pda(ics07_program_id);
    let (old_consensus, _) =
        Ics07Tendermint::consensus_state_pda(old_consensus_height, ics07_program_id);
    let (new_consensus, _) =
        Ics07Tendermint::consensus_state_pda(new_consensus_height, ics07_program_id);

    let mut account_metas = vec![
        AccountMeta::new(client_state, false),
        AccountMeta::new_readonly(old_consensus, false),
        AccountMeta::new(new_consensus, false),
        AccountMeta::new(*payer, true),
        AccountMeta::new_readonly(solana_system_interface::program::ID, false),
        AccountMeta::new_readonly(sysvar::instructions::ID, false),
    ];

    for pda in header_chunk_pdas {
        account_metas.push(AccountMeta::new(*pda, false));
    }
    for pda in sig_verify_pdas {
        account_metas.push(AccountMeta::new_readonly(*pda, false));
    }

    let assemble_ix = Instruction {
        program_id: *ics07_program_id,
        accounts: account_metas,
        data,
    };

    vec![
        ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
        ComputeBudgetInstruction::set_compute_unit_price(COMPUTE_UNIT_PRICE),
        ComputeBudgetInstruction::request_heap_frame(256 * 1024),
        assemble_ix,
    ]
}
