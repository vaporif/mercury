use borsh::BorshSerialize;
#[cfg(feature = "cosmos")]
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

/// Picks enough signatures to exceed 2/3 voting power, or errors if the
/// given signatures cannot reach that threshold.
#[cfg(feature = "cosmos")]
pub fn select_minimal_signatures(
    header: &ibc_client_tendermint::types::Header,
    signatures: &[crate::ibc_types::SignatureData],
) -> eyre::Result<Vec<crate::ibc_types::SignatureData>> {
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

    eyre::ensure!(
        accumulated >= threshold,
        "signatures carry {accumulated} of {total_power} voting power, need {threshold}"
    );
    Ok(selected)
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

    use crate::accounts::{
        ANCHOR_DISCRIMINATOR_LEN, AccessManager, anchor_instruction_discriminator,
    };
    use crate::ibc_types::SignatureData;

    fn decode<T: borsh::BorshDeserialize>(ix: &Instruction, method: &str) -> T {
        let (disc, body) = ix.data.split_at(ANCHOR_DISCRIMINATOR_LEN);
        assert_eq!(disc, anchor_instruction_discriminator(method));
        borsh::from_slice(body).unwrap()
    }

    fn sample_sig() -> SignatureData {
        SignatureData {
            signature_hash: [3u8; 32],
            pubkey: [4u8; 32],
            msg: b"vote".to_vec(),
            signature: [5u8; 64],
        }
    }

    #[test]
    fn upload_header_chunk_targets_per_submitter_chunk_pda() {
        let (ics07, payer, am) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let ix = upload_header_chunk(&ics07, &payer, 1_234, 2, vec![7; 10], &am).unwrap();

        let (height, idx, chunk): (u64, u8, Vec<u8>) = decode(&ix, "upload_header_chunk");
        assert_eq!((height, idx, chunk), (1_234, 2, vec![7; 10]));
        assert_eq!(ix.program_id, ics07);
        assert_eq!(
            ix.accounts,
            vec![
                AccountMeta::new(
                    Ics07Tendermint::header_chunk_pda(&payer, 1_234, 2, &ics07).0,
                    false
                ),
                AccountMeta::new_readonly(Ics07Tendermint::client_state_pda(&ics07).0, false),
                AccountMeta::new_readonly(Ics07Tendermint::app_state_pda(&ics07).0, false),
                AccountMeta::new_readonly(AccessManager::pda(&am).0, false),
                AccountMeta::new(payer, true),
                AccountMeta::new_readonly(sysvar::instructions::ID, false),
                AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            ]
        );
    }

    #[test]
    fn assemble_and_update_client_layout() {
        let (ics07, payer, am) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let chunks = [Pubkey::new_unique(), Pubkey::new_unique()];
        let sigs = [Pubkey::new_unique()];
        let ixs = assemble_and_update_client(&ics07, &payer, 10, 20, &chunks, &sigs, &am).unwrap();

        assert_eq!(
            ixs[..3],
            [
                ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                ComputeBudgetInstruction::set_compute_unit_price(COMPUTE_UNIT_PRICE),
                ComputeBudgetInstruction::request_heap_frame(256 * 1024),
            ]
        );
        let ix = &ixs[3];
        assert_eq!(ixs.len(), 4);
        let (target, count, trusted): (u64, u8, u64) = decode(ix, "assemble_and_update_client");
        assert_eq!((target, count, trusted), (20, 2, 10));

        assert_eq!(ix.program_id, ics07);
        assert_eq!(
            ix.accounts,
            vec![
                AccountMeta::new(Ics07Tendermint::client_state_pda(&ics07).0, false),
                AccountMeta::new_readonly(Ics07Tendermint::app_state_pda(&ics07).0, false),
                AccountMeta::new_readonly(AccessManager::pda(&am).0, false),
                AccountMeta::new_readonly(
                    Ics07Tendermint::consensus_state_pda(10, &ics07).0,
                    false
                ),
                AccountMeta::new(Ics07Tendermint::consensus_state_pda(20, &ics07).0, false),
                AccountMeta::new(payer, true),
                AccountMeta::new_readonly(solana_system_interface::program::ID, false),
                AccountMeta::new_readonly(sysvar::instructions::ID, false),
                // Header chunks are closed by the program, sig-verify PDAs only read.
                AccountMeta::new(chunks[0], false),
                AccountMeta::new(chunks[1], false),
                AccountMeta::new_readonly(sigs[0], false),
            ]
        );
    }

    #[test]
    fn assemble_and_update_client_caps_chunk_count_at_u8() {
        let (ics07, payer, am) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let max = vec![Pubkey::new_unique(); usize::from(u8::MAX)];
        let ixs = assemble_and_update_client(&ics07, &payer, 1, 2, &max, &[], &am).unwrap();
        let (_, count, _): (u64, u8, u64) = decode(&ixs[3], "assemble_and_update_client");
        assert_eq!(count, u8::MAX);

        let over = vec![Pubkey::new_unique(); usize::from(u8::MAX) + 1];
        let err = assemble_and_update_client(&ics07, &payer, 1, 2, &over, &[], &am).unwrap_err();
        assert!(
            err.to_string().contains("too many header chunks: 256"),
            "{err}"
        );
    }

    #[test]
    fn pre_verify_signature_records_at_signature_hash_pda() {
        let (ics07, payer, am) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let sig = sample_sig();
        let ix = pre_verify_signature(&ics07, &payer, &sig, &am).unwrap();

        let decoded: SignatureData = decode(&ix, "pre_verify_signature");
        assert_eq!(decoded.signature_hash, sig.signature_hash);
        assert_eq!(decoded.pubkey, sig.pubkey);
        assert_eq!(decoded.msg, sig.msg);
        assert_eq!(decoded.signature, sig.signature);
        assert_eq!(
            ix.accounts,
            vec![
                AccountMeta::new_readonly(sysvar::instructions::ID, false),
                AccountMeta::new(
                    Ics07Tendermint::sig_verify_pda(&sig.signature_hash, &ics07).0,
                    false
                ),
                AccountMeta::new_readonly(Ics07Tendermint::app_state_pda(&ics07).0, false),
                AccountMeta::new_readonly(AccessManager::pda(&am).0, false),
                AccountMeta::new(payer, true),
                AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            ]
        );
    }

    #[test]
    fn pre_verify_signature_instructions_place_precompile_before_verify() {
        // The program introspects the instruction right before it, so the
        // ed25519 precompile must immediately precede `pre_verify_signature`.
        let (ics07, payer, am) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let sig = sample_sig();
        let ixs = pre_verify_signature_instructions(&ics07, &payer, &sig, &am).unwrap();
        assert_eq!(
            ixs,
            vec![
                ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                ComputeBudgetInstruction::set_compute_unit_price(COMPUTE_UNIT_PRICE),
                build_ed25519_instruction(&sig),
                pre_verify_signature(&ics07, &payer, &sig, &am).unwrap(),
            ]
        );
    }

    #[test]
    fn cleanup_incomplete_upload_closes_every_pda_to_payer() {
        let (ics07, payer) = (Pubkey::new_unique(), Pubkey::new_unique());
        let pdas = [Pubkey::new_unique(), Pubkey::new_unique()];
        let ix = cleanup_incomplete_upload(&ics07, &payer, &pdas).unwrap();

        assert_eq!(ix.program_id, ics07);
        assert_eq!(
            ix.data,
            anchor_instruction_discriminator("cleanup_incomplete_upload")
        );
        assert_eq!(
            ix.accounts,
            vec![
                AccountMeta::new(payer, true),
                AccountMeta::new(pdas[0], false),
                AccountMeta::new(pdas[1], false),
            ]
        );

        let empty = cleanup_incomplete_upload(&ics07, &payer, &[]).unwrap();
        assert_eq!(empty.accounts, vec![AccountMeta::new(payer, true)]);
    }

    #[cfg(feature = "cosmos")]
    mod cosmos {
        use ibc_client_tendermint::types::Header;
        use ibc_core_client_types::Height as IbcHeight;
        use tendermint::account::Id as AccountId;
        use tendermint::block::commit_sig::CommitSig;
        use tendermint::block::header::{Header as TmHeader, Version};
        use tendermint::block::parts::Header as PartSetHeader;
        use tendermint::block::signed_header::SignedHeader;
        use tendermint::block::{Commit, Height, Id as BlockId, Round};
        use tendermint::chain::Id as ChainId;
        use tendermint::hash::{Algorithm, Hash};
        use tendermint::validator::{Info as ValidatorInfo, Set as ValidatorSet};
        use tendermint::{PublicKey, Time};

        use super::*;

        fn validator(key: u8, power: u32) -> ValidatorInfo {
            let pk = PublicKey::from_raw_ed25519(&[key; 32]).unwrap();
            ValidatorInfo {
                address: AccountId::from(pk),
                pub_key: pk,
                power: power.into(),
                name: None,
                proposer_priority: 0.into(),
            }
        }

        fn hash(byte: u8) -> Hash {
            Hash::from_bytes(Algorithm::Sha256, &[byte; 32]).unwrap()
        }

        fn time() -> Time {
            Time::from_unix_timestamp(1_700_000_000, 0).unwrap()
        }

        fn commit_sig(key: u8, flag: &str) -> CommitSig {
            let addr = AccountId::from(PublicKey::from_raw_ed25519(&[key; 32]).unwrap());
            let sig = Some(tendermint::Signature::try_from([key; 64].as_slice()).unwrap());
            match flag {
                "commit" => CommitSig::BlockIdFlagCommit {
                    validator_address: addr,
                    timestamp: time(),
                    signature: sig,
                },
                "unsigned" => CommitSig::BlockIdFlagCommit {
                    validator_address: addr,
                    timestamp: time(),
                    signature: None,
                },
                "nil" => CommitSig::BlockIdFlagNil {
                    validator_address: addr,
                    timestamp: time(),
                    signature: sig,
                },
                _ => CommitSig::BlockIdFlagAbsent,
            }
        }

        fn header(validators: &[(u8, u32)], sigs: Vec<CommitSig>) -> Header {
            let vals: Vec<_> = validators.iter().map(|&(k, p)| validator(k, p)).collect();
            let set = ValidatorSet::new(vals.clone(), Some(vals[0].clone()));
            let block_id = BlockId {
                hash: hash(0xAA),
                part_set_header: PartSetHeader::new(1, hash(0xBB)).unwrap(),
            };
            let tm_header = TmHeader {
                version: Version { block: 11, app: 1 },
                chain_id: ChainId::try_from("test-chain-1".to_string()).unwrap(),
                height: Height::try_from(100u64).unwrap(),
                time: time(),
                last_block_id: Some(block_id),
                last_commit_hash: Some(hash(1)),
                data_hash: Some(hash(2)),
                validators_hash: hash(3),
                next_validators_hash: hash(4),
                consensus_hash: hash(5),
                app_hash: vec![6; 32].try_into().unwrap(),
                last_results_hash: Some(hash(7)),
                evidence_hash: Some(hash(8)),
                proposer_address: vals[0].address,
            };
            let commit = Commit {
                height: Height::try_from(100u64).unwrap(),
                round: Round::from(0u16),
                block_id,
                signatures: sigs,
            };
            Header {
                signed_header: SignedHeader::new(tm_header, commit).unwrap(),
                validator_set: set.clone(),
                trusted_height: IbcHeight::new(0, 99).unwrap(),
                trusted_next_validator_set: set,
            }
        }

        fn sig_for(key: u8) -> SignatureData {
            SignatureData {
                signature_hash: [0; 32],
                pubkey: [key; 32],
                msg: vec![],
                signature: [key; 64],
            }
        }

        #[test]
        fn extracts_only_committed_known_signatures_by_power_desc() {
            let h = header(
                &[(1, 10), (2, 20), (3, 30), (4, 5)],
                vec![
                    commit_sig(1, "commit"),
                    commit_sig(2, "nil"),
                    commit_sig(0, "absent"),
                    commit_sig(3, "commit"),
                    commit_sig(4, "unsigned"),
                    commit_sig(9, "commit"), // not in the validator set
                ],
            );
            let sigs = extract_signatures_from_header(&h);

            let pubkeys: Vec<_> = sigs.iter().map(|s| s.pubkey).collect();
            assert_eq!(pubkeys, vec![[3; 32], [1; 32]]);
            for s in &sigs {
                assert_eq!(s.signature, [s.pubkey[0]; 64]);
                let expected: [u8; 32] = Sha256::digest(s.signature).into();
                assert_eq!(s.signature_hash, expected);
                assert!(
                    s.msg.windows(12).any(|w| w == b"test-chain-1"),
                    "sign bytes must commit to the chain id"
                );
            }
        }

        #[test]
        fn selects_until_strictly_above_two_thirds() {
            // Total 60 -> threshold 41.
            let h = header(&[(1, 10), (2, 20), (3, 30)], vec![]);
            let selected =
                select_minimal_signatures(&h, &[sig_for(3), sig_for(2), sig_for(1)]).unwrap();
            let keys: Vec<_> = selected.iter().map(|s| s.pubkey[0]).collect();
            assert_eq!(keys, vec![3, 2]);
        }

        #[test]
        fn exactly_two_thirds_is_not_enough() {
            // Total 3 -> threshold 3; two of three equal validators is exactly 2/3.
            let h = header(&[(1, 1), (2, 1), (3, 1)], vec![]);
            let selected =
                select_minimal_signatures(&h, &[sig_for(1), sig_for(2), sig_for(3)]).unwrap();
            assert_eq!(selected.len(), 3);
            assert!(select_minimal_signatures(&h, &[sig_for(1), sig_for(2)]).is_err());
        }

        #[test]
        fn skips_signatures_from_unknown_validators() {
            let h = header(&[(1, 10), (2, 20)], vec![]);
            let selected =
                select_minimal_signatures(&h, &[sig_for(9), sig_for(2), sig_for(1)]).unwrap();
            let keys: Vec<_> = selected.iter().map(|s| s.pubkey[0]).collect();
            assert_eq!(keys, vec![2, 1]);
        }

        #[test]
        fn errors_when_quorum_unreachable() {
            let h = header(&[(1, 10), (2, 20), (3, 30)], vec![]);
            let err = select_minimal_signatures(&h, &[sig_for(3), sig_for(1)]).unwrap_err();
            assert!(err.to_string().contains("40 of 60"), "{err}");
        }
    }
}
