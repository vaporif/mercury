//! Test-only fixtures: a deployed router seeded into a `FakeRpc`, and
//! Tendermint headers for update-client tests.

use ibc::core::client::types::Height as IbcHeight;
use ibc_client_tendermint::types::Header as TmIbcHeader;
use mercury_solana::accounts::Ics26Router;
use mercury_solana::chain::testing::test_chain;
use mercury_solana::rpc::testing::{FakeRpc, anchor_account};
use solana_sdk::pubkey::Pubkey;
use tendermint::block::{Commit, CommitSig, Header as BlockHeader, Height as TmHeight, Round};
use tendermint::validator::{Info as ValidatorInfo, Set as ValidatorSet};

use crate::wrapper::SolanaAdapter;

pub const PORT: &str = "transfer";

// Router state: version, AccessManagerState { access_manager, pending }, paused.
type WireRouterState = (u8, ([u8; 32], Option<[u8; 32]>), bool);

pub struct Deployed {
    pub ics26: Pubkey,
    pub ics07: Pubkey,
    pub am: Pubkey,
    pub app: Pubkey,
}

/// Seeds the router, `client` and `PORT` app accounts the builders resolve.
pub fn deployed(client: &str) -> (Deployed, FakeRpc) {
    let d = Deployed {
        ics26: Pubkey::new_unique(),
        ics07: Pubkey::new_unique(),
        am: Pubkey::new_unique(),
        app: Pubkey::new_unique(),
    };
    let router: WireRouterState = (1, (d.am.to_bytes(), None), false);
    let rpc = FakeRpc::default()
        .with_account(
            Ics26Router::router_state_pda(&d.ics26).0,
            anchor_account("RouterState", &router),
        )
        .with_account(
            Ics26Router::client_pda(client, &d.ics26).0,
            anchor_account("Client", &(1u8, client.to_string(), d.ics07.to_bytes())),
        )
        .with_account(
            Ics26Router::ibc_app_pda(PORT, &d.ics26).0,
            anchor_account("IBCApp", &(1u8, PORT.to_string(), d.app.to_bytes())),
        );
    (d, rpc)
}

pub fn adapter(d: &Deployed, rpc: FakeRpc) -> SolanaAdapter {
    let mut chain = test_chain(d.ics26, rpc);
    chain.ics07_program_id = Some(d.ics07);
    SolanaAdapter(chain)
}

fn validator(key: u8, power: u32) -> ValidatorInfo {
    let pk = tendermint::PublicKey::from_raw_ed25519(&[key; 32]).unwrap();
    ValidatorInfo::new(pk, power.into())
}

fn commit_sig(v: &ValidatorInfo, sig_byte: u8) -> CommitSig {
    CommitSig::BlockIdFlagCommit {
        validator_address: v.address,
        timestamp: tendermint::Time::from_unix_timestamp(1_700_000_000, 0).unwrap(),
        signature: Some(tendermint::Signature::try_from([sig_byte; 64].as_slice()).unwrap()),
    }
}

/// A header at `target` with validators of the given `powers`. Validator `i`
/// signs (with signature bytes `i + 10`) when listed in `signing`.
pub fn tm_header(powers: &[u32], signing: &[usize], target: u64, trusted: u64) -> TmIbcHeader {
    let validators: Vec<_> = powers
        .iter()
        .enumerate()
        .map(|(i, &p)| validator(u8::try_from(i + 1).unwrap(), p))
        .collect();
    let set = ValidatorSet::new(validators.clone(), Some(validators[0].clone()));
    let header = BlockHeader {
        version: tendermint::block::header::Version { block: 11, app: 1 },
        chain_id: "cosmoshub-4".parse().unwrap(),
        height: TmHeight::try_from(target).unwrap(),
        time: tendermint::Time::from_unix_timestamp(1_700_000_000, 0).unwrap(),
        last_block_id: None,
        last_commit_hash: None,
        data_hash: None,
        validators_hash: set.hash(),
        next_validators_hash: set.hash(),
        consensus_hash: tendermint::Hash::Sha256([3; 32]),
        app_hash: vec![4; 32].try_into().unwrap(),
        last_results_hash: None,
        evidence_hash: None,
        proposer_address: validators[0].address,
    };
    let commit = Commit {
        height: header.height,
        round: Round::from(0u16),
        block_id: tendermint::block::Id {
            hash: header.hash(),
            part_set_header: tendermint::block::parts::Header::new(
                1,
                tendermint::Hash::Sha256([5; 32]),
            )
            .unwrap(),
        },
        signatures: signing
            .iter()
            .map(|&i| commit_sig(&validators[i], u8::try_from(i + 10).unwrap()))
            .collect(),
    };
    TmIbcHeader {
        signed_header: tendermint::block::signed_header::SignedHeader::new(header, commit).unwrap(),
        validator_set: set.clone(),
        trusted_height: IbcHeight::new(4, trusted).unwrap(),
        trusted_next_validator_set: set,
    }
}
