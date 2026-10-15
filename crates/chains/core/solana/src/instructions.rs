pub mod chunking;
pub mod client;
pub mod signatures;

use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::sysvar;

use crate::accounts::{self, AccessManager, IbcApp, Ics07Tendermint, Ics26Router};
use crate::ibc_types;

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

pub struct PacketParams<'a> {
    pub ics26_program_id: &'a Pubkey,
    pub payer: &'a Pubkey,
    pub client_id: &'a str,
    pub port: &'a str,
    pub sequence: u64,
    pub ics07_program_id: &'a Pubkey,
    pub consensus_height: u64,
    pub access_manager_program_id: &'a Pubkey,
    pub app_program_id: &'a Pubkey,
}

pub fn recv_packet(
    params: &PacketParams<'_>,
    msg: &ibc_types::MsgRecvPacket,
    chunk_account_metas: Vec<AccountMeta>,
) -> eyre::Result<Instruction> {
    let data = accounts::encode_anchor_instruction("recv_packet", msg)?;

    let (router_state, _) = Ics26Router::router_state_pda(params.ics26_program_id);
    let (access_manager, _) = AccessManager::pda(params.access_manager_program_id);
    let (ibc_app, _) = Ics26Router::ibc_app_pda(params.port, params.ics26_program_id);
    let (packet_receipt, _) =
        Ics26Router::packet_receipt_pda(params.client_id, params.sequence, params.ics26_program_id);
    let (packet_ack, _) =
        Ics26Router::packet_ack_pda(params.client_id, params.sequence, params.ics26_program_id);
    let (app_state, _) = IbcApp::state_pda(params.app_program_id);
    let (client_pda, _) = Ics26Router::client_pda(params.client_id, params.ics26_program_id);
    let (client_state, _) = Ics07Tendermint::client_state_pda(params.ics07_program_id);
    let (consensus_state, _) =
        Ics07Tendermint::consensus_state_pda(params.consensus_height, params.ics07_program_id);

    let mut accounts = vec![
        AccountMeta::new_readonly(router_state, false),
        AccountMeta::new_readonly(access_manager, false),
        AccountMeta::new_readonly(ibc_app, false),
        AccountMeta::new(packet_receipt, false),
        AccountMeta::new(packet_ack, false),
        AccountMeta::new_readonly(*params.app_program_id, false),
        AccountMeta::new(app_state, false),
        AccountMeta::new(*params.payer, true),
        AccountMeta::new_readonly(solana_system_interface::program::ID, false),
        AccountMeta::new_readonly(sysvar::instructions::ID, false),
        AccountMeta::new_readonly(client_pda, false),
        AccountMeta::new_readonly(*params.ics07_program_id, false),
        AccountMeta::new_readonly(client_state, false),
        AccountMeta::new_readonly(consensus_state, false),
    ];
    accounts.extend(chunk_account_metas);

    Ok(Instruction {
        program_id: *params.ics26_program_id,
        accounts,
        data,
    })
}

pub fn ack_packet(
    params: &PacketParams<'_>,
    msg: &ibc_types::MsgAckPacket,
    chunk_account_metas: Vec<AccountMeta>,
) -> eyre::Result<Instruction> {
    let data = accounts::encode_anchor_instruction("ack_packet", msg)?;

    let (router_state, _) = Ics26Router::router_state_pda(params.ics26_program_id);
    let (access_manager, _) = AccessManager::pda(params.access_manager_program_id);
    let (ibc_app, _) = Ics26Router::ibc_app_pda(params.port, params.ics26_program_id);
    let (packet_commitment, _) = Ics26Router::packet_commitment_pda(
        params.client_id,
        params.sequence,
        params.ics26_program_id,
    );
    let (app_state, _) = IbcApp::state_pda(params.app_program_id);
    let (client_pda, _) = Ics26Router::client_pda(params.client_id, params.ics26_program_id);
    let (client_state, _) = Ics07Tendermint::client_state_pda(params.ics07_program_id);
    let (consensus_state, _) =
        Ics07Tendermint::consensus_state_pda(params.consensus_height, params.ics07_program_id);

    let mut accounts = vec![
        AccountMeta::new_readonly(router_state, false),
        AccountMeta::new_readonly(access_manager, false),
        AccountMeta::new_readonly(ibc_app, false),
        AccountMeta::new(packet_commitment, false),
        AccountMeta::new_readonly(*params.app_program_id, false),
        AccountMeta::new(app_state, false),
        AccountMeta::new(*params.payer, true),
        AccountMeta::new_readonly(solana_system_interface::program::ID, false),
        AccountMeta::new_readonly(sysvar::instructions::ID, false),
        AccountMeta::new_readonly(client_pda, false),
        AccountMeta::new_readonly(*params.ics07_program_id, false),
        AccountMeta::new_readonly(client_state, false),
        AccountMeta::new_readonly(consensus_state, false),
    ];
    accounts.extend(chunk_account_metas);

    Ok(Instruction {
        program_id: *params.ics26_program_id,
        accounts,
        data,
    })
}

pub fn timeout_packet(
    params: &PacketParams<'_>,
    msg: &ibc_types::MsgTimeoutPacket,
    chunk_account_metas: Vec<AccountMeta>,
) -> eyre::Result<Instruction> {
    let data = accounts::encode_anchor_instruction("timeout_packet", msg)?;

    let (router_state, _) = Ics26Router::router_state_pda(params.ics26_program_id);
    let (access_manager, _) = AccessManager::pda(params.access_manager_program_id);
    let (ibc_app, _) = Ics26Router::ibc_app_pda(params.port, params.ics26_program_id);
    let (packet_commitment, _) = Ics26Router::packet_commitment_pda(
        params.client_id,
        params.sequence,
        params.ics26_program_id,
    );
    let (app_state, _) = IbcApp::state_pda(params.app_program_id);
    let (client_pda, _) = Ics26Router::client_pda(params.client_id, params.ics26_program_id);
    let (client_state, _) = Ics07Tendermint::client_state_pda(params.ics07_program_id);
    let (consensus_state, _) =
        Ics07Tendermint::consensus_state_pda(params.consensus_height, params.ics07_program_id);

    let mut accounts = vec![
        AccountMeta::new_readonly(router_state, false),
        AccountMeta::new_readonly(access_manager, false),
        AccountMeta::new_readonly(ibc_app, false),
        AccountMeta::new(packet_commitment, false),
        AccountMeta::new_readonly(*params.app_program_id, false),
        AccountMeta::new(app_state, false),
        AccountMeta::new(*params.payer, true),
        AccountMeta::new_readonly(solana_system_interface::program::ID, false),
        AccountMeta::new_readonly(sysvar::instructions::ID, false),
        AccountMeta::new_readonly(client_pda, false),
        AccountMeta::new_readonly(*params.ics07_program_id, false),
        AccountMeta::new_readonly(client_state, false),
        AccountMeta::new_readonly(consensus_state, false),
    ];
    accounts.extend(chunk_account_metas);

    Ok(Instruction {
        program_id: *params.ics26_program_id,
        accounts,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ibc_types::{Delivery, MsgPacket, MsgPayload, MsgProof};

    struct Keys {
        ics26: Pubkey,
        payer: Pubkey,
        ics07: Pubkey,
        am: Pubkey,
        app: Pubkey,
    }

    impl Keys {
        fn new() -> Self {
            Self {
                ics26: Pubkey::new_unique(),
                payer: Pubkey::new_unique(),
                ics07: Pubkey::new_unique(),
                am: Pubkey::new_unique(),
                app: Pubkey::new_unique(),
            }
        }

        fn params(&self) -> PacketParams<'_> {
            PacketParams {
                ics26_program_id: &self.ics26,
                payer: &self.payer,
                client_id: "07-tendermint-0",
                port: "transfer",
                sequence: 5,
                ics07_program_id: &self.ics07,
                consensus_height: 77,
                access_manager_program_id: &self.am,
                app_program_id: &self.app,
            }
        }
    }

    fn packet() -> MsgPacket {
        MsgPacket {
            sequence: 5,
            source_client: "08-wasm-0".into(),
            dest_client: "07-tendermint-0".into(),
            timeout_timestamp: 1_000,
            payloads: vec![MsgPayload {
                source_port: "transfer".into(),
                dest_port: "transfer".into(),
                version: "ics20-1".into(),
                encoding: "json".into(),
                data: Delivery::Chunked { total_chunks: 2 },
            }],
        }
    }

    fn proof() -> MsgProof {
        MsgProof {
            height: 77,
            data: Delivery::Inline { data: vec![1, 2] },
        }
    }

    fn meta(pubkey: Pubkey, writable: bool, signer: bool) -> AccountMeta {
        AccountMeta {
            pubkey,
            is_signer: signer,
            is_writable: writable,
        }
    }

    fn assert_data(ix: &Instruction, method: &str, msg: &impl borsh::BorshSerialize) {
        let (disc, body) = ix.data.split_at(accounts::ANCHOR_DISCRIMINATOR_LEN);
        assert_eq!(disc, accounts::anchor_instruction_discriminator(method));
        assert_eq!(body, borsh::to_vec(msg).unwrap());
    }

    /// Accounts shared by ack and timeout, which only differ in data.
    fn expected_source_side_accounts(k: &Keys) -> Vec<AccountMeta> {
        let p = k.params();
        vec![
            meta(Ics26Router::router_state_pda(&k.ics26).0, false, false),
            meta(AccessManager::pda(&k.am).0, false, false),
            meta(Ics26Router::ibc_app_pda(p.port, &k.ics26).0, false, false),
            meta(
                Ics26Router::packet_commitment_pda(p.client_id, p.sequence, &k.ics26).0,
                true,
                false,
            ),
            meta(k.app, false, false),
            meta(IbcApp::state_pda(&k.app).0, true, false),
            meta(k.payer, true, true),
            meta(solana_system_interface::program::ID, false, false),
            meta(sysvar::instructions::ID, false, false),
            meta(
                Ics26Router::client_pda(p.client_id, &k.ics26).0,
                false,
                false,
            ),
            meta(k.ics07, false, false),
            meta(Ics07Tendermint::client_state_pda(&k.ics07).0, false, false),
            meta(
                Ics07Tendermint::consensus_state_pda(p.consensus_height, &k.ics07).0,
                false,
                false,
            ),
        ]
    }

    #[test]
    fn with_compute_budget_prepends_limit_then_price() {
        let ix = Instruction::new_with_bytes(Pubkey::new_unique(), &[9], vec![]);
        let ixs = with_compute_budget(ix.clone());
        assert_eq!(
            ixs,
            vec![
                ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                ComputeBudgetInstruction::set_compute_unit_price(COMPUTE_UNIT_PRICE),
                ix,
            ]
        );
    }

    #[test]
    fn recv_packet_account_layout() {
        let k = Keys::new();
        let p = k.params();
        let msg = ibc_types::MsgRecvPacket {
            packet: packet(),
            proof: proof(),
        };
        let ix = recv_packet(&p, &msg, vec![]).unwrap();

        assert_eq!(ix.program_id, k.ics26);
        assert_data(&ix, "recv_packet", &msg);
        let expected = vec![
            meta(Ics26Router::router_state_pda(&k.ics26).0, false, false),
            meta(AccessManager::pda(&k.am).0, false, false),
            meta(
                Ics26Router::ibc_app_pda("transfer", &k.ics26).0,
                false,
                false,
            ),
            meta(
                Ics26Router::packet_receipt_pda("07-tendermint-0", 5, &k.ics26).0,
                true,
                false,
            ),
            meta(
                Ics26Router::packet_ack_pda("07-tendermint-0", 5, &k.ics26).0,
                true,
                false,
            ),
            meta(k.app, false, false),
            meta(IbcApp::state_pda(&k.app).0, true, false),
            meta(k.payer, true, true),
            meta(solana_system_interface::program::ID, false, false),
            meta(sysvar::instructions::ID, false, false),
            meta(
                Ics26Router::client_pda("07-tendermint-0", &k.ics26).0,
                false,
                false,
            ),
            meta(k.ics07, false, false),
            meta(Ics07Tendermint::client_state_pda(&k.ics07).0, false, false),
            meta(
                Ics07Tendermint::consensus_state_pda(77, &k.ics07).0,
                false,
                false,
            ),
        ];
        assert_eq!(ix.accounts, expected);
    }

    #[test]
    fn ack_packet_account_layout() {
        let k = Keys::new();
        let msg = ibc_types::MsgAckPacket {
            packet: packet(),
            acknowledgement: vec![0xAB; 3],
            proof: proof(),
        };
        let ix = ack_packet(&k.params(), &msg, vec![]).unwrap();
        assert_eq!(ix.program_id, k.ics26);
        assert_data(&ix, "ack_packet", &msg);
        assert_eq!(ix.accounts, expected_source_side_accounts(&k));
    }

    #[test]
    fn timeout_packet_account_layout() {
        let k = Keys::new();
        let msg = ibc_types::MsgTimeoutPacket {
            packet: packet(),
            proof: proof(),
        };
        let ix = timeout_packet(&k.params(), &msg, vec![]).unwrap();
        assert_eq!(ix.program_id, k.ics26);
        assert_data(&ix, "timeout_packet", &msg);
        assert_eq!(ix.accounts, expected_source_side_accounts(&k));
    }

    #[test]
    fn chunk_accounts_are_appended_in_order() {
        let k = Keys::new();
        let chunks = vec![
            AccountMeta::new(Pubkey::new_unique(), false),
            AccountMeta::new(Pubkey::new_unique(), false),
        ];
        let recv = recv_packet(
            &k.params(),
            &ibc_types::MsgRecvPacket {
                packet: packet(),
                proof: proof(),
            },
            chunks.clone(),
        )
        .unwrap();
        assert_eq!(recv.accounts.len(), 14 + 2);
        assert_eq!(&recv.accounts[14..], chunks.as_slice());

        let timeout = timeout_packet(
            &k.params(),
            &ibc_types::MsgTimeoutPacket {
                packet: packet(),
                proof: proof(),
            },
            chunks.clone(),
        )
        .unwrap();
        assert_eq!(&timeout.accounts[13..], chunks.as_slice());
    }

    #[test]
    fn packet_instructions_have_distinct_discriminators() {
        let k = Keys::new();
        let p = k.params();
        let recv = recv_packet(
            &p,
            &ibc_types::MsgRecvPacket {
                packet: packet(),
                proof: proof(),
            },
            vec![],
        )
        .unwrap();
        let timeout = timeout_packet(
            &p,
            &ibc_types::MsgTimeoutPacket {
                packet: packet(),
                proof: proof(),
            },
            vec![],
        )
        .unwrap();
        // Same borsh body, so only the discriminator tells the router what to do.
        assert_eq!(recv.data[8..], timeout.data[8..]);
        assert_ne!(recv.data[..8], timeout.data[..8]);
    }
}
