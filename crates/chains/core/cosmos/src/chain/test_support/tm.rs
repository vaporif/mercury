//! `CometBFT` fixtures: validator sets, headers whose hashes are internally
//! consistent, and JSON-RPC result bodies built by serializing the same
//! `tendermint-rpc` response types the client deserializes.

use serde_json::{json, Value};
use tendermint::abci::{types::ExecTxResult, Event, EventAttributeIndexExt};
use tendermint::block::{self, parts, signed_header::SignedHeader, Commit, Header, Height};
use tendermint::validator::{Info, Set};
use tendermint::{account, AppHash, Hash, Time};
use tendermint_rpc::endpoint::{commit, validators};

use super::rpc::{ok, Reply};

pub const CHAIN_ID: &str = "testchain-1";
pub const GENESIS_UNIX: i64 = 1_700_000_000;

pub fn validator(seed: u8, power: u32) -> Info {
    let secret = secp256k1::SecretKey::from_secret_bytes([seed; 32]).unwrap();
    let pk = secp256k1::PublicKey::from_secret_key(&secret);
    let pk = tendermint::PublicKey::from_raw_secp256k1(&pk.serialize()).unwrap();
    Info::new(pk, power.into())
}

pub fn validators(seeds: &[u8]) -> Vec<Info> {
    seeds.iter().map(|s| validator(*s, 10)).collect()
}

pub fn set_hash(vals: &[Info]) -> Hash {
    Set::new(vals.to_vec(), None).hash()
}

pub fn block_time(height: u64) -> Time {
    Time::from_unix_timestamp(GENESIS_UNIX + i64::try_from(height).unwrap(), 0).unwrap()
}

pub fn header(height: u64, app_hash: u8, vals: &[Info], next_vals: &[Info]) -> Header {
    Header {
        version: block::header::Version { block: 11, app: 0 },
        chain_id: CHAIN_ID.parse().unwrap(),
        height: Height::try_from(height).unwrap(),
        time: block_time(height),
        last_block_id: None,
        last_commit_hash: None,
        data_hash: None,
        validators_hash: set_hash(vals),
        next_validators_hash: set_hash(next_vals),
        consensus_hash: Hash::Sha256([0x0c; 32]),
        app_hash: AppHash::try_from(vec![app_hash; 32]).unwrap(),
        last_results_hash: None,
        evidence_hash: None,
        proposer_address: vals
            .first()
            .map_or_else(|| account::Id::new([0; 20]), |v| v.address),
    }
}

pub fn signed_header(header: Header) -> SignedHeader {
    let commit = Commit {
        height: header.height,
        round: block::Round::default(),
        block_id: block::Id {
            hash: header.hash(),
            part_set_header: parts::Header::new(1, Hash::Sha256([0x0d; 32])).unwrap(),
        },
        signatures: vec![],
    };
    SignedHeader::new(header, commit).unwrap()
}

/// Header with a commit carrying real secp256k1 precommit signatures from every
/// validator built from `seeds` (see `validator`), so a light client
/// verifier accepts it.
pub fn signed_header_by(header: Header, seeds: &[u8], power: u32) -> SignedHeader {
    use tendermint::block::CommitSig;
    use tendermint::vote::{Type, ValidatorIndex, Vote};

    let unsigned = signed_header(header.clone());
    let block_id = unsigned.commit.block_id;
    let set = Set::new(seeds.iter().map(|s| validator(*s, power)).collect(), None);
    let signatures = set
        .validators()
        .iter()
        .enumerate()
        .map(|(idx, info)| {
            let seed = *seeds
                .iter()
                .find(|s| validator(**s, power).address == info.address)
                .unwrap();
            let timestamp = header.time;
            let vote = Vote {
                vote_type: Type::Precommit,
                height: header.height,
                round: block::Round::default(),
                block_id: Some(block_id),
                timestamp: Some(timestamp),
                validator_address: info.address,
                validator_index: ValidatorIndex::try_from(u32::try_from(idx).unwrap()).unwrap(),
                signature: None,
                extension: vec![],
                extension_signature: None,
            };
            let sign_bytes = vote.into_signable_vec(header.chain_id.clone());
            let digest: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(&sign_bytes).into();
            let secret = secp256k1::SecretKey::from_secret_bytes([seed; 32]).unwrap();
            let sig = secp256k1::ecdsa::sign(secp256k1::Message::from_digest(digest), &secret);
            CommitSig::BlockIdFlagCommit {
                validator_address: info.address,
                timestamp,
                signature: tendermint::Signature::new(sig.serialize_compact()).unwrap(),
            }
        })
        .collect();
    let commit = Commit {
        signatures,
        ..unsigned.commit
    };
    SignedHeader::new(header, commit).unwrap()
}

pub fn commit_reply(header: Header) -> Reply {
    ok(&commit::Response {
        signed_header: signed_header(header),
        canonical: true,
    })
}

pub fn validators_reply(height: u64, vals: &[Info]) -> Reply {
    ok(&validators::Response::new(
        Height::try_from(height).unwrap(),
        vals.to_vec(),
        i32::try_from(vals.len()).unwrap(),
    ))
}

pub fn status_json(height: u64, catching_up: bool, tx_index_on: bool) -> Value {
    let hash = "AB".repeat(32);
    json!({
        "node_info": {
            "protocol_version": {"p2p": "8", "block": "11", "app": "0"},
            "id": "aa".repeat(20),
            "listen_addr": "tcp://0.0.0.0:26656",
            "network": CHAIN_ID,
            "version": "0.38.12",
            "channels": "40202122233038606100",
            "moniker": "mock",
            "other": {
                "tx_index": if tx_index_on { "on" } else { "off" },
                "rpc_address": "tcp://0.0.0.0:26657"
            }
        },
        "sync_info": {
            "latest_block_hash": hash,
            "latest_app_hash": hash,
            "latest_block_height": height.to_string(),
            "latest_block_time": block_time(height),
            "earliest_block_hash": hash,
            "earliest_app_hash": hash,
            "earliest_block_height": "1",
            "earliest_block_time": block_time(1),
            "catching_up": catching_up
        },
        "validator_info": validator(1, 10)
    })
}

pub fn event(kind: &str, attrs: &[(&str, &str)]) -> Event {
    Event::new(kind, attrs.iter().map(|(k, v)| (*k, *v).index()))
}

pub fn exec_result(code: u32, log: &str, events: Vec<Event>) -> ExecTxResult {
    ExecTxResult {
        code: code.into(),
        log: log.to_string(),
        events,
        ..Default::default()
    }
}

/// `tx` / `tx_search` entry for a transaction included at `height`.
pub fn tx_entry(hash: Hash, height: u64, result: ExecTxResult) -> Value {
    serde_json::to_value(tendermint_rpc::endpoint::tx::Response {
        hash,
        height: Height::try_from(height).unwrap(),
        index: 0,
        tx_result: result,
        tx: vec![],
        proof: None,
    })
    .unwrap()
}

pub fn tx_search_reply(txs: Vec<Value>) -> Reply {
    let total = txs.len().to_string();
    Reply::Result(json!({"txs": Value::Array(txs), "total_count": total}))
}

pub fn abci_reply(value: Vec<u8>, proof: Option<tendermint::merkle::proof::ProofOps>) -> Reply {
    ok(&tendermint_rpc::endpoint::abci_query::Response {
        response: tendermint_rpc::endpoint::abci_query::AbciQuery {
            value,
            proof,
            ..Default::default()
        },
    })
}

/// Proof ops whose data decodes as ICS-23 existence proofs for `keys`.
pub fn proof_ops(keys: &[&[u8]]) -> tendermint::merkle::proof::ProofOps {
    use ibc_proto::ics23::{commitment_proof, CommitmentProof, ExistenceProof};
    use prost::Message as _;

    let ops = keys
        .iter()
        .map(|key| tendermint::merkle::proof::ProofOp {
            field_type: "ics23:iavl".into(),
            key: key.to_vec(),
            data: CommitmentProof {
                proof: Some(commitment_proof::Proof::Exist(ExistenceProof {
                    key: key.to_vec(),
                    value: b"v".to_vec(),
                    leaf: None,
                    path: vec![],
                })),
            }
            .encode_to_vec(),
        })
        .collect();
    tendermint::merkle::proof::ProofOps { ops }
}

/// `block` result in the `CometBFT` 0.38 wire format.
pub fn block_reply(header: Header) -> Reply {
    use tendermint_rpc::endpoint::block::v0_38::DialectResponse;

    let block_id = signed_header(header.clone()).commit.block_id;
    let block = tendermint::Block::new(header, vec![], tendermint::evidence::List::default(), None);
    ok(&DialectResponse {
        block_id,
        block: block.into(),
    })
}
