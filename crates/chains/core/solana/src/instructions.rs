pub mod chunking;
pub mod signatures;

use borsh::BorshSerialize;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::sysvar;

use crate::accounts::{self, AccessManager, IbcApp, Ics07Tendermint, Ics26Router};

pub const COMPUTE_UNIT_LIMIT: u32 = 1_400_000;
pub const COMPUTE_UNIT_PRICE: u64 = 1000;

#[must_use]
pub fn with_compute_budget(ix: Instruction) -> Vec<Instruction> {
    vec![
        ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
        ComputeBudgetInstruction::set_compute_unit_price(COMPUTE_UNIT_PRICE),
        ix,
    ]
}

#[derive(BorshSerialize)]
struct RegisterCounterpartyArgs {
    client_id: String,
    counterparty_client_id: String,
    merkle_prefix: Vec<u8>,
}

#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn register_counterparty(
    ics26_program_id: &Pubkey,
    payer: &Pubkey,
    client_id: &str,
    counterparty_client_id: &str,
    merkle_prefix: &[u8],
    access_manager_program_id: &Pubkey,
) -> Instruction {
    let args = RegisterCounterpartyArgs {
        client_id: client_id.to_string(),
        counterparty_client_id: counterparty_client_id.to_string(),
        merkle_prefix: merkle_prefix.to_vec(),
    };
    let data = accounts::encode_anchor_instruction("register_counterparty", &args);

    let (router_state, _) = Ics26Router::router_state_pda(ics26_program_id);
    let (client_pda, _) = Ics26Router::client_pda(client_id, ics26_program_id);
    let (access_manager, _) = AccessManager::pda(access_manager_program_id);

    Instruction {
        program_id: *ics26_program_id,
        accounts: vec![
            AccountMeta::new_readonly(router_state, false),
            AccountMeta::new(client_pda, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(access_manager, false),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            AccountMeta::new_readonly(sysvar::instructions::ID, false),
        ],
        data,
    }
}

#[derive(BorshSerialize)]
pub struct MsgRecvPacket {
    pub packet_bytes: Vec<u8>,
    pub proof_commitment: Vec<u8>,
    pub proof_height_revision_number: u64,
    pub proof_height_revision_height: u64,
}

#[must_use]
#[allow(clippy::too_many_arguments, clippy::missing_panics_doc)]
pub fn recv_packet(
    ics26_program_id: &Pubkey,
    payer: &Pubkey,
    msg: &MsgRecvPacket,
    dest_client_id: &str,
    dest_port: &str,
    sequence: u64,
    ics07_program_id: &Pubkey,
    consensus_height: u64,
    access_manager_program_id: &Pubkey,
    app_program_id: &Pubkey,
) -> Instruction {
    let data = accounts::encode_anchor_instruction("recv_packet", msg);

    let (router_state, _) = Ics26Router::router_state_pda(ics26_program_id);
    let (access_manager, _) = AccessManager::pda(access_manager_program_id);
    let (ibc_app, _) = Ics26Router::ibc_app_pda(dest_port, ics26_program_id);
    let (packet_receipt, _) =
        Ics26Router::packet_receipt_pda(dest_client_id, sequence, ics26_program_id);
    let (packet_ack, _) = Ics26Router::packet_ack_pda(dest_client_id, sequence, ics26_program_id);
    let (app_state, _) = IbcApp::state_pda(app_program_id);
    let (client_pda, _) = Ics26Router::client_pda(dest_client_id, ics26_program_id);
    let (client_state, _) = Ics07Tendermint::client_state_pda(ics07_program_id);
    let (consensus_state, _) =
        Ics07Tendermint::consensus_state_pda(consensus_height, ics07_program_id);

    Instruction {
        program_id: *ics26_program_id,
        accounts: vec![
            AccountMeta::new_readonly(router_state, false),
            AccountMeta::new_readonly(access_manager, false),
            AccountMeta::new_readonly(ibc_app, false),
            AccountMeta::new(packet_receipt, false),
            AccountMeta::new(packet_ack, false),
            AccountMeta::new_readonly(*app_program_id, false),
            AccountMeta::new(app_state, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            AccountMeta::new_readonly(sysvar::instructions::ID, false),
            AccountMeta::new_readonly(client_pda, false),
            AccountMeta::new_readonly(*ics07_program_id, false),
            AccountMeta::new_readonly(client_state, false),
            AccountMeta::new_readonly(consensus_state, false),
        ],
        data,
    }
}

#[derive(BorshSerialize)]
pub struct MsgAckPacket {
    pub packet_bytes: Vec<u8>,
    pub acknowledgement: Vec<u8>,
    pub proof_acked: Vec<u8>,
    pub proof_height_revision_number: u64,
    pub proof_height_revision_height: u64,
}

#[must_use]
#[allow(clippy::too_many_arguments, clippy::missing_panics_doc)]
pub fn ack_packet(
    ics26_program_id: &Pubkey,
    payer: &Pubkey,
    msg: &MsgAckPacket,
    source_client_id: &str,
    source_port: &str,
    sequence: u64,
    ics07_program_id: &Pubkey,
    consensus_height: u64,
    access_manager_program_id: &Pubkey,
    app_program_id: &Pubkey,
) -> Instruction {
    let data = accounts::encode_anchor_instruction("ack_packet", msg);

    let (router_state, _) = Ics26Router::router_state_pda(ics26_program_id);
    let (access_manager, _) = AccessManager::pda(access_manager_program_id);
    let (ibc_app, _) = Ics26Router::ibc_app_pda(source_port, ics26_program_id);
    let (packet_commitment, _) =
        Ics26Router::packet_commitment_pda(source_client_id, sequence, ics26_program_id);
    let (app_state, _) = IbcApp::state_pda(app_program_id);
    let (client_pda, _) = Ics26Router::client_pda(source_client_id, ics26_program_id);
    let (client_state, _) = Ics07Tendermint::client_state_pda(ics07_program_id);
    let (consensus_state, _) =
        Ics07Tendermint::consensus_state_pda(consensus_height, ics07_program_id);

    Instruction {
        program_id: *ics26_program_id,
        accounts: vec![
            AccountMeta::new_readonly(router_state, false),
            AccountMeta::new_readonly(access_manager, false),
            AccountMeta::new_readonly(ibc_app, false),
            AccountMeta::new(packet_commitment, false),
            AccountMeta::new_readonly(*app_program_id, false),
            AccountMeta::new(app_state, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            AccountMeta::new_readonly(sysvar::instructions::ID, false),
            AccountMeta::new_readonly(client_pda, false),
            AccountMeta::new_readonly(*ics07_program_id, false),
            AccountMeta::new_readonly(client_state, false),
            AccountMeta::new_readonly(consensus_state, false),
        ],
        data,
    }
}

#[derive(BorshSerialize)]
pub struct MsgTimeoutPacket {
    pub packet_bytes: Vec<u8>,
    pub proof_unreceived: Vec<u8>,
    pub proof_height_revision_number: u64,
    pub proof_height_revision_height: u64,
}

#[must_use]
#[allow(clippy::too_many_arguments, clippy::missing_panics_doc)]
pub fn timeout_packet(
    ics26_program_id: &Pubkey,
    payer: &Pubkey,
    msg: &MsgTimeoutPacket,
    source_client_id: &str,
    source_port: &str,
    sequence: u64,
    ics07_program_id: &Pubkey,
    consensus_height: u64,
    access_manager_program_id: &Pubkey,
    app_program_id: &Pubkey,
) -> Instruction {
    let data = accounts::encode_anchor_instruction("timeout_packet", msg);

    let (router_state, _) = Ics26Router::router_state_pda(ics26_program_id);
    let (access_manager, _) = AccessManager::pda(access_manager_program_id);
    let (ibc_app, _) = Ics26Router::ibc_app_pda(source_port, ics26_program_id);
    let (packet_commitment, _) =
        Ics26Router::packet_commitment_pda(source_client_id, sequence, ics26_program_id);
    let (app_state, _) = IbcApp::state_pda(app_program_id);
    let (client_pda, _) = Ics26Router::client_pda(source_client_id, ics26_program_id);
    let (client_state, _) = Ics07Tendermint::client_state_pda(ics07_program_id);
    let (consensus_state, _) =
        Ics07Tendermint::consensus_state_pda(consensus_height, ics07_program_id);

    Instruction {
        program_id: *ics26_program_id,
        accounts: vec![
            AccountMeta::new_readonly(router_state, false),
            AccountMeta::new_readonly(access_manager, false),
            AccountMeta::new_readonly(ibc_app, false),
            AccountMeta::new(packet_commitment, false),
            AccountMeta::new_readonly(*app_program_id, false),
            AccountMeta::new(app_state, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            AccountMeta::new_readonly(sysvar::instructions::ID, false),
            AccountMeta::new_readonly(client_pda, false),
            AccountMeta::new_readonly(*ics07_program_id, false),
            AccountMeta::new_readonly(client_state, false),
            AccountMeta::new_readonly(consensus_state, false),
        ],
        data,
    }
}
