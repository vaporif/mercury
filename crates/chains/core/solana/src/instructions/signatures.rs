use borsh::BorshSerialize;
use sha2::{Digest, Sha256};
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

pub fn upload_header_chunk(
    ics07_program_id: &Pubkey,
    payer: &Pubkey,
    target_height: u64,
    chunk_index: u8,
    chunk_data: Vec<u8>,
    access_manager_program_id: &Pubkey,
) -> eyre::Result<Instruction> {
    let args = UploadHeaderChunkArgs {
        target_height,
        chunk_index,
        chunk_data,
    };
    let data = accounts::encode_anchor_instruction("upload_header_chunk", &args)?;

    let (header_chunk_pda, _) =
        Ics07Tendermint::header_chunk_pda(payer, target_height, chunk_index, ics07_program_id);
    let (client_state_pda, _) = Ics07Tendermint::client_state_pda(ics07_program_id);
    let (app_state_pda, _) = Ics07Tendermint::app_state_pda(ics07_program_id);
    let (access_manager_pda, _) = crate::accounts::AccessManager::pda(access_manager_program_id);

    Ok(Instruction {
        program_id: *ics07_program_id,
        accounts: vec![
            AccountMeta::new(header_chunk_pda, false),
            AccountMeta::new_readonly(client_state_pda, false),
            AccountMeta::new_readonly(app_state_pda, false),
            AccountMeta::new_readonly(access_manager_pda, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(sysvar::instructions::ID, false),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
        ],
        data,
    })
}

#[derive(BorshSerialize)]
struct AssembleAndUpdateClientArgs {
    target_height: u64,
    chunk_count: u8,
    trusted_height: u64,
}

pub fn assemble_and_update_client(
    ics07_program_id: &Pubkey,
    payer: &Pubkey,
    trusted_consensus_height: u64,
    new_consensus_height: u64,
    header_chunk_pdas: &[Pubkey],
    sig_verify_pdas: &[Pubkey],
    access_manager_program_id: &Pubkey,
) -> eyre::Result<Vec<Instruction>> {
    let chunk_count = u8::try_from(header_chunk_pdas.len())
        .map_err(|_| eyre::eyre!("too many header chunks: {}", header_chunk_pdas.len()))?;
    let args = AssembleAndUpdateClientArgs {
        target_height: new_consensus_height,
        chunk_count,
        trusted_height: trusted_consensus_height,
    };

    let mut data =
        accounts::anchor_instruction_discriminator("assemble_and_update_client").to_vec();
    args.serialize(&mut data)?;

    let (client_state, _) = Ics07Tendermint::client_state_pda(ics07_program_id);
    let (app_state, _) = Ics07Tendermint::app_state_pda(ics07_program_id);
    let (access_manager, _) = crate::accounts::AccessManager::pda(access_manager_program_id);
    let (trusted_consensus, _) =
        Ics07Tendermint::consensus_state_pda(trusted_consensus_height, ics07_program_id);
    let (new_consensus, _) =
        Ics07Tendermint::consensus_state_pda(new_consensus_height, ics07_program_id);

    let mut account_metas = vec![
        AccountMeta::new(client_state, false),
        AccountMeta::new_readonly(app_state, false),
        AccountMeta::new_readonly(access_manager, false),
        AccountMeta::new_readonly(trusted_consensus, false),
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

    Ok(vec![
        ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
        ComputeBudgetInstruction::set_compute_unit_price(COMPUTE_UNIT_PRICE),
        ComputeBudgetInstruction::request_heap_frame(256 * 1024),
        assemble_ix,
    ])
}

pub fn pre_verify_signature(
    ics07_program_id: &Pubkey,
    payer: &Pubkey,
    signature: &crate::ibc_types::SignatureData,
    access_manager_program_id: &Pubkey,
) -> eyre::Result<Instruction> {
    let data = accounts::encode_anchor_instruction("pre_verify_signature", signature)?;

    let (app_state, _) = Ics07Tendermint::app_state_pda(ics07_program_id);
    let (access_manager, _) = crate::accounts::AccessManager::pda(access_manager_program_id);
    let (sig_verify_pda, _) =
        Ics07Tendermint::sig_verify_pda(&signature.signature_hash, ics07_program_id);

    Ok(Instruction {
        program_id: *ics07_program_id,
        accounts: vec![
            AccountMeta::new_readonly(sysvar::instructions::ID, false),
            AccountMeta::new(sig_verify_pda, false),
            AccountMeta::new_readonly(app_state, false),
            AccountMeta::new_readonly(access_manager, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
        ],
        data,
    })
}

/// # Panics
/// If the message length exceeds `u16::MAX`.
#[must_use]
pub fn build_ed25519_instruction(sig: &crate::ibc_types::SignatureData) -> Instruction {
    // The SDK builder truncates the length with `as u16`; reject oversized messages instead.
    assert!(
        u16::try_from(sig.msg.len()).is_ok(),
        "message length {} exceeds u16::MAX",
        sig.msg.len()
    );
    solana_ed25519_program::new_ed25519_instruction_with_signature(
        &sig.msg,
        &sig.signature,
        &sig.pubkey,
    )
}

/// Ed25519 verify + on-chain record.
pub fn pre_verify_signature_instructions(
    ics07_program_id: &Pubkey,
    payer: &Pubkey,
    signature: &crate::ibc_types::SignatureData,
    access_manager_program_id: &Pubkey,
) -> eyre::Result<Vec<Instruction>> {
    let ed25519_ix = build_ed25519_instruction(signature);
    let verify_ix = pre_verify_signature(
        ics07_program_id,
        payer,
        signature,
        access_manager_program_id,
    )?;

    Ok(vec![
        ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
        ComputeBudgetInstruction::set_compute_unit_price(COMPUTE_UNIT_PRICE),
        ed25519_ix,
        verify_ix,
    ])
}

/// Sorted by voting power descending.
///
/// # Panics
/// If a validator index exceeds `u32::MAX`.
#[must_use]
#[cfg(feature = "cosmos")]
pub fn extract_signatures_from_header(
    header: &ibc_client_tendermint::types::Header,
) -> Vec<crate::ibc_types::SignatureData> {
    use tendermint::vote::Vote;

    let signed_header = &header.signed_header;
    let commit = &signed_header.commit;
    let validators = &header.validator_set;

    let chain_id = signed_header.header.chain_id.clone();
    let mut sig_data: Vec<(u64, crate::ibc_types::SignatureData)> = Vec::new();

    for commit_sig in &commit.signatures {
        let tendermint::block::commit_sig::CommitSig::BlockIdFlagCommit {
            validator_address,
            timestamp,
            signature,
        } = commit_sig
        else {
            continue;
        };

        let Some(signature_bytes) = signature else {
            continue;
        };

        let Some(idx) = validators
            .validators()
            .iter()
            .position(|v| v.address == *validator_address)
        else {
            continue;
        };

        let validator = &validators.validators()[idx];
        let tendermint::PublicKey::Ed25519(pk) = validator.pub_key else {
            continue;
        };

        let pubkey_bytes: [u8; 32] = pk.as_bytes().try_into().unwrap_or([0u8; 32]);

        let Some(validator_index) = u32::try_from(idx)
            .ok()
            .and_then(|i| tendermint::vote::ValidatorIndex::try_from(i).ok())
        else {
            continue;
        };
        let vote = Vote {
            vote_type: tendermint::vote::Type::Precommit,
            height: commit.height,
            round: commit.round,
            block_id: Some(commit.block_id),
            timestamp: Some(*timestamp),
            validator_address: *validator_address,
            validator_index,
            signature: signature.clone(),
            extension: Vec::new(),
            extension_signature: None,
        };
        let sign_bytes = vote.into_signable_vec(chain_id.clone());

        let sig_bytes_arr: [u8; 64] = signature_bytes.as_bytes().try_into().unwrap_or([0u8; 64]);

        let signature_hash: [u8; 32] = Sha256::digest(sig_bytes_arr).into();

        sig_data.push((
            validator.power.value(),
            crate::ibc_types::SignatureData {
                signature_hash,
                pubkey: pubkey_bytes,
                msg: sign_bytes,
                signature: sig_bytes_arr,
            },
        ));
    }

    sig_data.sort_by_key(|b| std::cmp::Reverse(b.0));
    sig_data.into_iter().map(|(_, s)| s).collect()
}

/// Picks enough signatures to exceed 2/3 voting power.
#[must_use]
#[cfg(feature = "cosmos")]
pub fn select_minimal_signatures(
    header: &ibc_client_tendermint::types::Header,
    signatures: &[crate::ibc_types::SignatureData],
) -> Vec<crate::ibc_types::SignatureData> {
    let total_power: u64 = header
        .validator_set
        .validators()
        .iter()
        .map(|v| v.power.value())
        .sum();
    let threshold = total_power * 2 / 3 + 1;

    let validators = header.validator_set.validators();
    let mut accumulated: u64 = 0;
    let mut selected = Vec::new();

    for sig in signatures {
        if accumulated >= threshold {
            break;
        }
        if let Some(v) = validators.iter().find(|v| {
            if let tendermint::PublicKey::Ed25519(pk) = v.pub_key {
                let pk_bytes: [u8; 32] = pk.as_bytes().try_into().unwrap_or([0u8; 32]);
                pk_bytes == sig.pubkey
            } else {
                false
            }
        }) {
            accumulated += v.power.value();
            selected.push(sig.clone());
        }
    }

    selected
}

/// Close header chunk and sig-verify PDAs via `cleanup_incomplete_upload`.
pub fn cleanup_incomplete_upload(
    ics07_program_id: &Pubkey,
    payer: &Pubkey,
    pdas: &[Pubkey],
) -> eyre::Result<Instruction> {
    let data = accounts::anchor_instruction_discriminator("cleanup_incomplete_upload").to_vec();

    let mut account_metas = vec![AccountMeta::new(*payer, true)];
    for pda in pdas {
        account_metas.push(AccountMeta::new(*pda, false));
    }

    Ok(Instruction {
        program_id: *ics07_program_id,
        accounts: account_metas,
        data,
    })
}

pub fn initialize_ics07(
    ics07_program_id: &Pubkey,
    payer: &Pubkey,
    client_state: &crate::ibc_types::ClientState,
    consensus_state: &crate::ibc_types::ConsensusState,
    access_manager_program_id: &Pubkey,
) -> eyre::Result<Instruction> {
    #[derive(BorshSerialize)]
    struct InitializeArgs {
        client_state: crate::ibc_types::ClientState,
        consensus_state: crate::ibc_types::ConsensusState,
        access_manager: Pubkey,
    }

    let args = InitializeArgs {
        client_state: client_state.clone(),
        consensus_state: consensus_state.clone(),
        access_manager: *access_manager_program_id,
    };
    let data = accounts::encode_anchor_instruction("initialize", &args)?;

    let (client_state_pda, _) = Ics07Tendermint::client_state_pda(ics07_program_id);
    let (consensus_state_pda, _) = Ics07Tendermint::consensus_state_pda(
        client_state.latest_height.revision_height,
        ics07_program_id,
    );
    let (app_state_pda, _) = Ics07Tendermint::app_state_pda(ics07_program_id);

    Ok(Instruction {
        program_id: *ics07_program_id,
        accounts: vec![
            AccountMeta::new(client_state_pda, false),
            AccountMeta::new(consensus_state_pda, false),
            AccountMeta::new(app_state_pda, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
        ],
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ed25519_instruction_layout() {
        use solana_ed25519_program::{DATA_START, SIGNATURE_OFFSETS_START};

        let sig = crate::ibc_types::SignatureData {
            signature_hash: [0u8; 32],
            pubkey: [2u8; 32],
            msg: b"hello".to_vec(),
            signature: [1u8; 64],
        };

        let ix = build_ed25519_instruction(&sig);

        assert_eq!(ix.program_id, solana_sdk::ed25519_program::ID);
        assert_eq!(ix.accounts, vec![]);
        assert_eq!(ix.data.len(), DATA_START + 64 + 32 + sig.msg.len());
        assert_eq!(ix.data[0], 1, "num signatures");

        // Offsets as read by the ed25519 precompile and ics07 pre_verify_signatures.
        let read_u16 = |field: usize| {
            let at = SIGNATURE_OFFSETS_START + field * 2;
            usize::from(u16::from_le_bytes([ix.data[at], ix.data[at + 1]]))
        };
        let (sig_off, pk_off, msg_off, msg_len) =
            (read_u16(0), read_u16(2), read_u16(4), read_u16(5));
        for ix_index in [read_u16(1), read_u16(3), read_u16(6)] {
            assert_eq!(ix_index, usize::from(u16::MAX), "data must be inline");
        }

        assert_eq!(&ix.data[sig_off..sig_off + 64], &sig.signature);
        assert_eq!(&ix.data[pk_off..pk_off + 32], &sig.pubkey);
        assert_eq!(msg_len, sig.msg.len());
        assert_eq!(&ix.data[msg_off..msg_off + msg_len], b"hello");
    }

    #[test]
    #[should_panic(expected = "exceeds u16::MAX")]
    fn ed25519_instruction_rejects_oversized_message() {
        let sig = crate::ibc_types::SignatureData {
            signature_hash: [0u8; 32],
            pubkey: [0u8; 32],
            msg: vec![0u8; usize::from(u16::MAX) + 1],
            signature: [0u8; 64],
        };
        let _ = build_ed25519_instruction(&sig);
    }
}
