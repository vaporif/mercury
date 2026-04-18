use borsh::BorshSerialize;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::sysvar;

use crate::accounts::{self, AccessManager, Ics07Tendermint, Ics26Router};
use crate::ibc_types::{ClientState, ConsensusState, CounterpartyInfo};

#[derive(BorshSerialize)]
struct AddClientArgs {
    client_id: String,
    counterparty_info: CounterpartyInfo,
}

pub fn add_client(
    ics26_program_id: &Pubkey,
    payer: &Pubkey,
    client_id: &str,
    counterparty_info: CounterpartyInfo,
    ics07_program_id: &Pubkey,
    access_manager_program_id: &Pubkey,
) -> eyre::Result<Instruction> {
    let args = AddClientArgs {
        client_id: client_id.to_string(),
        counterparty_info,
    };
    let data = accounts::encode_anchor_instruction("add_client", &args)?;

    let (router_state, _) = Ics26Router::router_state_pda(ics26_program_id);
    let (access_manager, _) = AccessManager::pda(access_manager_program_id);
    let (client, _) = Ics26Router::client_pda(client_id, ics26_program_id);

    Ok(Instruction {
        program_id: *ics26_program_id,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(router_state, false),
            AccountMeta::new_readonly(access_manager, false),
            AccountMeta::new(client, false),
            AccountMeta::new_readonly(*ics07_program_id, false),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            AccountMeta::new_readonly(sysvar::instructions::ID, false),
        ],
        data,
    })
}

#[derive(BorshSerialize)]
struct InitializeIcs07Args {
    client_state: ClientState,
    consensus_state: ConsensusState,
    access_manager: Pubkey,
}

pub fn initialize_ics07(
    ics07_program_id: &Pubkey,
    payer: &Pubkey,
    client_state: &ClientState,
    consensus_state: &ConsensusState,
    access_manager_program_id: &Pubkey,
) -> eyre::Result<Instruction> {
    let args = InitializeIcs07Args {
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

#[derive(BorshSerialize)]
struct AddIbcAppArgs {
    port_id: String,
}

pub fn add_ibc_app(
    ics26_program_id: &Pubkey,
    payer: &Pubkey,
    authority: &Pubkey,
    port_id: &str,
    app_program_id: &Pubkey,
    access_manager_program_id: &Pubkey,
) -> eyre::Result<Instruction> {
    let args = AddIbcAppArgs {
        port_id: port_id.to_string(),
    };
    let data = accounts::encode_anchor_instruction("add_ibc_app", &args)?;

    let (router_state, _) = Ics26Router::router_state_pda(ics26_program_id);
    let (access_manager, _) = AccessManager::pda(access_manager_program_id);
    let (ibc_app, _) = Ics26Router::ibc_app_pda(port_id, ics26_program_id);

    Ok(Instruction {
        program_id: *ics26_program_id,
        accounts: vec![
            AccountMeta::new_readonly(router_state, false),
            AccountMeta::new_readonly(access_manager, false),
            AccountMeta::new(ibc_app, false),
            AccountMeta::new_readonly(*app_program_id, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new(*authority, true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            AccountMeta::new_readonly(sysvar::instructions::ID, false),
        ],
        data,
    })
}
