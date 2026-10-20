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
    let program_data_pda = solana_loader_v3_interface::get_program_data_address(ics07_program_id);

    Ok(Instruction {
        program_id: *ics07_program_id,
        accounts: vec![
            AccountMeta::new(client_state_pda, false),
            AccountMeta::new(consensus_state_pda, false),
            AccountMeta::new(app_state_pda, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            AccountMeta::new_readonly(program_data_pda, false),
            AccountMeta::new_readonly(*payer, true),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{ANCHOR_DISCRIMINATOR_LEN, anchor_instruction_discriminator};
    use crate::ibc_types::IbcHeight;

    fn decode<T: borsh::BorshDeserialize>(ix: &Instruction, method: &str) -> T {
        let (disc, body) = ix.data.split_at(ANCHOR_DISCRIMINATOR_LEN);
        assert_eq!(disc, anchor_instruction_discriminator(method));
        borsh::from_slice(body).unwrap()
    }

    fn client_state(latest: u64) -> ClientState {
        ClientState {
            chain_id: "cosmoshub-4".into(),
            trust_level_numerator: 1,
            trust_level_denominator: 3,
            trusting_period: 100,
            unbonding_period: 200,
            max_clock_drift: 10,
            frozen_height: IbcHeight {
                revision_number: 0,
                revision_height: 0,
            },
            latest_height: IbcHeight {
                revision_number: 4,
                revision_height: latest,
            },
        }
    }

    fn consensus_state() -> ConsensusState {
        ConsensusState {
            timestamp: 1_700_000_000,
            root: [1; 32],
            next_validators_hash: [2; 32],
        }
    }

    #[test]
    fn add_client_encodes_args_and_accounts() {
        let (ics26, payer, ics07, am) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let counterparty = CounterpartyInfo {
            client_id: "08-wasm-3".into(),
            merkle_prefix: vec![b"ibc".to_vec(), vec![]],
        };
        let ix = add_client(&ics26, &payer, "07-tendermint-0", counterparty, &ics07, &am).unwrap();

        let (client_id, (cp_client, prefix)): (String, (String, Vec<Vec<u8>>)) =
            decode(&ix, "add_client");
        assert_eq!(client_id, "07-tendermint-0");
        assert_eq!(cp_client, "08-wasm-3");
        assert_eq!(prefix, vec![b"ibc".to_vec(), vec![]]);

        assert_eq!(ix.program_id, ics26);
        assert_eq!(
            ix.accounts,
            vec![
                AccountMeta::new(payer, true),
                AccountMeta::new_readonly(Ics26Router::router_state_pda(&ics26).0, false),
                AccountMeta::new_readonly(AccessManager::pda(&am).0, false),
                AccountMeta::new(Ics26Router::client_pda("07-tendermint-0", &ics26).0, false),
                AccountMeta::new_readonly(ics07, false),
                AccountMeta::new_readonly(solana_system_interface::program::ID, false),
                AccountMeta::new_readonly(sysvar::instructions::ID, false),
            ]
        );
    }

    #[test]
    fn initialize_ics07_seeds_consensus_state_at_latest_height() {
        let (ics07, payer, am) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let cs = client_state(555);
        let ix = initialize_ics07(&ics07, &payer, &cs, &consensus_state(), &am).unwrap();

        let (decoded_cs, decoded_cons, access_manager): (ClientState, ConsensusState, [u8; 32]) =
            decode(&ix, "initialize");
        assert_eq!(decoded_cs.chain_id, "cosmoshub-4");
        assert_eq!(decoded_cs.latest_height.revision_height, 555);
        assert_eq!(decoded_cons.root, [1; 32]);
        assert_eq!(access_manager, am.to_bytes());

        assert_eq!(ix.program_id, ics07);
        assert_eq!(
            ix.accounts,
            vec![
                AccountMeta::new(Ics07Tendermint::client_state_pda(&ics07).0, false),
                AccountMeta::new(Ics07Tendermint::consensus_state_pda(555, &ics07).0, false),
                AccountMeta::new(Ics07Tendermint::app_state_pda(&ics07).0, false),
                AccountMeta::new(payer, true),
                AccountMeta::new_readonly(solana_system_interface::program::ID, false),
                AccountMeta::new_readonly(
                    solana_loader_v3_interface::get_program_data_address(&ics07),
                    false
                ),
                AccountMeta::new_readonly(payer, true),
            ]
        );
    }

    #[test]
    fn add_ibc_app_requires_payer_and_authority_signatures() {
        let (ics26, payer, authority, app, am) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let ix = add_ibc_app(&ics26, &payer, &authority, "transfer", &app, &am).unwrap();

        let port: String = decode(&ix, "add_ibc_app");
        assert_eq!(port, "transfer");
        assert_eq!(ix.program_id, ics26);
        assert_eq!(
            ix.accounts,
            vec![
                AccountMeta::new_readonly(Ics26Router::router_state_pda(&ics26).0, false),
                AccountMeta::new_readonly(AccessManager::pda(&am).0, false),
                AccountMeta::new(Ics26Router::ibc_app_pda("transfer", &ics26).0, false),
                AccountMeta::new_readonly(app, false),
                AccountMeta::new(payer, true),
                AccountMeta::new(authority, true),
                AccountMeta::new_readonly(solana_system_interface::program::ID, false),
                AccountMeta::new_readonly(sysvar::instructions::ID, false),
            ]
        );
    }
}
