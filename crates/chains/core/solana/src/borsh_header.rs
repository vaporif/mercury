use borsh::BorshSerialize;
use ibc_client_tendermint::types::Header;
use tendermint::PublicKey;
use tendermint::block::commit_sig::CommitSig;
use tendermint::block::signed_header::SignedHeader;
use tendermint::block::{Commit, Header as TmHeader, Id as BlockId};
use tendermint::validator::{Info as ValidatorInfo, Set as ValidatorSet};

#[derive(BorshSerialize)]
pub struct BorshHeader {
    pub signed_header: BorshSignedHeader,
    pub validator_set: BorshValidatorSet,
    pub trusted_height: BorshHeight,
    pub trusted_next_validator_set: BorshValidatorSet,
}

#[derive(BorshSerialize)]
pub struct BorshSignedHeader {
    pub header: BorshBlockHeader,
    pub commit: BorshCommit,
}

#[derive(BorshSerialize)]
pub struct BorshBlockHeader {
    pub version: BorshConsensusVersion,
    pub chain_id: String,
    pub height: u64,
    pub time: BorshTimestamp,
    pub last_block_id: Option<BorshBlockId>,
    pub last_commit_hash: Option<Vec<u8>>,
    pub data_hash: Option<Vec<u8>>,
    pub validators_hash: Vec<u8>,
    pub next_validators_hash: Vec<u8>,
    pub consensus_hash: Vec<u8>,
    pub app_hash: Vec<u8>,
    pub last_results_hash: Option<Vec<u8>>,
    pub evidence_hash: Option<Vec<u8>>,
    pub proposer_address: Vec<u8>,
}

#[derive(BorshSerialize)]
pub struct BorshConsensusVersion {
    pub block: u64,
    pub app: u64,
}

#[derive(BorshSerialize)]
pub struct BorshTimestamp {
    pub secs: i64,
    pub nanos: i32,
}

#[derive(BorshSerialize)]
pub struct BorshBlockId {
    pub hash: Vec<u8>,
    pub part_set_header: BorshPartSetHeader,
}

#[derive(BorshSerialize)]
pub struct BorshPartSetHeader {
    pub total: u32,
    pub hash: Vec<u8>,
}

#[derive(BorshSerialize)]
pub struct BorshCommit {
    pub height: u64,
    pub round: u16,
    pub block_id: BorshBlockId,
    pub signatures: Vec<BorshCommitSig>,
}

#[derive(BorshSerialize)]
pub enum BorshCommitSig {
    BlockIdFlagAbsent,
    BlockIdFlagCommit {
        validator_address: [u8; 20],
        timestamp: BorshTimestamp,
        signature: [u8; 64],
    },
    BlockIdFlagNil {
        validator_address: [u8; 20],
        timestamp: BorshTimestamp,
        signature: [u8; 64],
    },
}

#[derive(BorshSerialize)]
pub struct BorshValidatorSet {
    pub validators: Vec<BorshValidator>,
    pub proposer: Option<BorshValidator>,
    pub total_voting_power: u64,
}

#[derive(BorshSerialize)]
pub struct BorshValidator {
    pub address: [u8; 20],
    pub pub_key: BorshPublicKey,
    pub voting_power: u64,
    pub proposer_priority: i64,
}

#[derive(BorshSerialize)]
pub enum BorshPublicKey {
    Ed25519([u8; 32]),
    Secp256k1([u8; 33]),
}

#[derive(BorshSerialize)]
pub struct BorshHeight {
    pub revision_number: u64,
    pub revision_height: u64,
}

#[must_use]
pub fn header_to_borsh(h: Header) -> BorshHeader {
    BorshHeader {
        signed_header: signed_header_to_borsh(h.signed_header),
        validator_set: validator_set_to_borsh(h.validator_set),
        trusted_height: BorshHeight {
            revision_number: h.trusted_height.revision_number(),
            revision_height: h.trusted_height.revision_height(),
        },
        trusted_next_validator_set: validator_set_to_borsh(h.trusted_next_validator_set),
    }
}

fn signed_header_to_borsh(sh: SignedHeader) -> BorshSignedHeader {
    BorshSignedHeader {
        header: block_header_to_borsh(sh.header),
        commit: commit_to_borsh(sh.commit),
    }
}

fn block_header_to_borsh(h: TmHeader) -> BorshBlockHeader {
    BorshBlockHeader {
        version: BorshConsensusVersion {
            block: h.version.block,
            app: h.version.app,
        },
        chain_id: h.chain_id.to_string(),
        height: h.height.value(),
        time: time_to_borsh(h.time),
        last_block_id: h.last_block_id.map(block_id_to_borsh),
        last_commit_hash: h.last_commit_hash.map(|h| h.as_bytes().to_vec()),
        data_hash: h.data_hash.map(|h| h.as_bytes().to_vec()),
        validators_hash: h.validators_hash.as_bytes().to_vec(),
        next_validators_hash: h.next_validators_hash.as_bytes().to_vec(),
        consensus_hash: h.consensus_hash.as_bytes().to_vec(),
        app_hash: h.app_hash.as_bytes().to_vec(),
        last_results_hash: h.last_results_hash.map(|h| h.as_bytes().to_vec()),
        evidence_hash: h.evidence_hash.map(|h| h.as_bytes().to_vec()),
        proposer_address: h.proposer_address.as_bytes().to_vec(),
    }
}

fn time_to_borsh(t: tendermint::Time) -> BorshTimestamp {
    BorshTimestamp {
        secs: t.unix_timestamp(),
        nanos: (t.unix_timestamp_nanos() % 1_000_000_000) as i32,
    }
}

fn block_id_to_borsh(bid: BlockId) -> BorshBlockId {
    BorshBlockId {
        hash: bid.hash.as_bytes().to_vec(),
        part_set_header: BorshPartSetHeader {
            total: bid.part_set_header.total,
            hash: bid.part_set_header.hash.as_bytes().to_vec(),
        },
    }
}

fn commit_to_borsh(c: Commit) -> BorshCommit {
    let mut signatures: Vec<BorshCommitSig> =
        c.signatures.into_iter().map(commit_sig_to_borsh).collect();

    #[allow(clippy::match_same_arms)]
    signatures.sort_unstable_by(|a, b| match (a, b) {
        (
            BorshCommitSig::BlockIdFlagCommit {
                validator_address: addr_a,
                ..
            },
            BorshCommitSig::BlockIdFlagCommit {
                validator_address: addr_b,
                ..
            },
        )
        | (
            BorshCommitSig::BlockIdFlagNil {
                validator_address: addr_a,
                ..
            },
            BorshCommitSig::BlockIdFlagNil {
                validator_address: addr_b,
                ..
            },
        )
        | (
            BorshCommitSig::BlockIdFlagCommit {
                validator_address: addr_a,
                ..
            },
            BorshCommitSig::BlockIdFlagNil {
                validator_address: addr_b,
                ..
            },
        )
        | (
            BorshCommitSig::BlockIdFlagNil {
                validator_address: addr_a,
                ..
            },
            BorshCommitSig::BlockIdFlagCommit {
                validator_address: addr_b,
                ..
            },
        ) => addr_a.cmp(addr_b),
        (BorshCommitSig::BlockIdFlagAbsent, BorshCommitSig::BlockIdFlagAbsent) => {
            std::cmp::Ordering::Equal
        }
        (BorshCommitSig::BlockIdFlagAbsent, _) => std::cmp::Ordering::Less,
        (_, BorshCommitSig::BlockIdFlagAbsent) => std::cmp::Ordering::Greater,
    });

    BorshCommit {
        height: c.height.value(),
        round: c.round.value() as u16,
        block_id: block_id_to_borsh(c.block_id),
        signatures,
    }
}

fn commit_sig_to_borsh(cs: CommitSig) -> BorshCommitSig {
    match cs {
        CommitSig::BlockIdFlagAbsent => BorshCommitSig::BlockIdFlagAbsent,
        CommitSig::BlockIdFlagCommit {
            validator_address,
            timestamp,
            signature,
        } => {
            let address_array: [u8; 20] = validator_address
                .as_bytes()
                .try_into()
                .expect("Validator address must be 20 bytes");
            let sig_array: [u8; 64] = signature.map_or([0u8; 64], |s| {
                s.as_bytes().try_into().expect("Signature must be 64 bytes")
            });
            BorshCommitSig::BlockIdFlagCommit {
                validator_address: address_array,
                timestamp: time_to_borsh(timestamp),
                signature: sig_array,
            }
        }
        CommitSig::BlockIdFlagNil {
            validator_address,
            timestamp,
            signature,
        } => {
            let address_array: [u8; 20] = validator_address
                .as_bytes()
                .try_into()
                .expect("Validator address must be 20 bytes");
            let sig_array: [u8; 64] = signature.map_or([0u8; 64], |s| {
                s.as_bytes().try_into().expect("Signature must be 64 bytes")
            });
            BorshCommitSig::BlockIdFlagNil {
                validator_address: address_array,
                timestamp: time_to_borsh(timestamp),
                signature: sig_array,
            }
        }
    }
}

fn validator_set_to_borsh(vs: ValidatorSet) -> BorshValidatorSet {
    BorshValidatorSet {
        validators: vs
            .validators()
            .iter()
            .cloned()
            .map(validator_to_borsh)
            .collect(),
        proposer: vs.proposer().clone().map(validator_to_borsh),
        total_voting_power: vs.total_voting_power().value(),
    }
}

fn validator_to_borsh(v: ValidatorInfo) -> BorshValidator {
    let address_array: [u8; 20] = v
        .address
        .as_bytes()
        .try_into()
        .expect("Validator address must be 20 bytes");
    BorshValidator {
        address: address_array,
        pub_key: match v.pub_key {
            PublicKey::Ed25519(bytes) => {
                let bytes_array: [u8; 32] = bytes
                    .as_bytes()
                    .try_into()
                    .expect("Ed25519 pubkey must be 32 bytes");
                BorshPublicKey::Ed25519(bytes_array)
            }
            _ => panic!("Only Ed25519 public keys are supported on Solana"),
        },
        voting_power: v.power.value(),
        proposer_priority: v.proposer_priority.value(),
    }
}
