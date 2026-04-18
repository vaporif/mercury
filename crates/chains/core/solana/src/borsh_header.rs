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
        validator_set: validator_set_to_borsh(&h.validator_set),
        trusted_height: BorshHeight {
            revision_number: h.trusted_height.revision_number(),
            revision_height: h.trusted_height.revision_height(),
        },
        trusted_next_validator_set: validator_set_to_borsh(&h.trusted_next_validator_set),
    }
}

fn signed_header_to_borsh(sh: SignedHeader) -> BorshSignedHeader {
    BorshSignedHeader {
        header: block_header_to_borsh(&sh.header),
        commit: commit_to_borsh(sh.commit),
    }
}

fn block_header_to_borsh(h: &TmHeader) -> BorshBlockHeader {
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
        #[allow(clippy::cast_possible_truncation)]
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

fn validator_set_to_borsh(vs: &ValidatorSet) -> BorshValidatorSet {
    BorshValidatorSet {
        validators: vs.validators().iter().map(validator_to_borsh).collect(),
        proposer: vs.proposer().as_ref().map(validator_to_borsh),
        total_voting_power: vs.total_voting_power().value(),
    }
}

fn validator_to_borsh(v: &ValidatorInfo) -> BorshValidator {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ibc_client_tendermint::types::Header;
    use ibc_core_client_types::Height as IbcHeight;
    use tendermint::Time;
    use tendermint::account::Id as AccountId;
    use tendermint::block::header::Version;
    use tendermint::block::parts::Header as PartSetHeader;
    use tendermint::block::{Height, Round};
    use tendermint::chain::Id as ChainId;
    use tendermint::hash::{Algorithm, Hash};

    fn make_validator(key_byte: u8, power: u32) -> ValidatorInfo {
        let pk = PublicKey::from_raw_ed25519(&[key_byte; 32]).unwrap();
        ValidatorInfo {
            address: AccountId::from(pk),
            pub_key: pk,
            power: power.into(),
            name: None,
            proposer_priority: 0.into(),
        }
    }

    fn make_time() -> Time {
        Time::from_unix_timestamp(1_700_000_000, 123_456_789).unwrap()
    }

    fn make_hash(byte: u8) -> Hash {
        Hash::from_bytes(Algorithm::Sha256, &[byte; 32]).unwrap()
    }

    fn make_block_id() -> BlockId {
        BlockId {
            hash: make_hash(0xAA),
            part_set_header: PartSetHeader::new(42, make_hash(0xBB)).unwrap(),
        }
    }

    fn make_commit_sig(key_byte: u8, flag: &str) -> CommitSig {
        let pk = PublicKey::from_raw_ed25519(&[key_byte; 32]).unwrap();
        let addr = AccountId::from(pk);
        let sig_bytes: [u8; 64] = [key_byte; 64];
        let sig = tendermint::Signature::try_from(sig_bytes.as_slice()).unwrap();
        match flag {
            "commit" => CommitSig::BlockIdFlagCommit {
                validator_address: addr,
                timestamp: make_time(),
                signature: Some(sig),
            },
            "nil" => CommitSig::BlockIdFlagNil {
                validator_address: addr,
                timestamp: make_time(),
                signature: Some(sig),
            },
            _ => CommitSig::BlockIdFlagAbsent,
        }
    }

    fn make_tm_header(proposer: &ValidatorInfo) -> TmHeader {
        TmHeader {
            version: Version { block: 11, app: 1 },
            chain_id: ChainId::try_from("test-chain-1".to_string()).unwrap(),
            height: Height::try_from(100u64).unwrap(),
            time: make_time(),
            last_block_id: Some(make_block_id()),
            last_commit_hash: Some(make_hash(0x01)),
            data_hash: Some(make_hash(0x02)),
            validators_hash: make_hash(0x03),
            next_validators_hash: make_hash(0x04),
            consensus_hash: make_hash(0x05),
            app_hash: vec![0x06; 32].try_into().unwrap(),
            last_results_hash: Some(make_hash(0x07)),
            evidence_hash: Some(make_hash(0x08)),
            proposer_address: proposer.address,
        }
    }

    fn make_commit(sigs: Vec<CommitSig>) -> Commit {
        Commit {
            height: Height::try_from(100u64).unwrap(),
            round: Round::try_from(1u16).unwrap(),
            block_id: make_block_id(),
            signatures: sigs,
        }
    }

    fn make_full_header() -> Header {
        let v1 = make_validator(1, 10);
        let v2 = make_validator(2, 20);
        let v3 = make_validator(3, 30);

        let validator_set =
            ValidatorSet::new(vec![v1.clone(), v2.clone(), v3.clone()], Some(v1.clone()));
        let trusted_set = ValidatorSet::new(vec![v1.clone(), v2.clone()], Some(v1.clone()));

        let tm_header = make_tm_header(&v1);
        let commit = make_commit(vec![
            make_commit_sig(3, "commit"),
            make_commit_sig(0, "absent"),
            make_commit_sig(1, "commit"),
        ]);

        let signed_header = SignedHeader::new(tm_header, commit).unwrap();

        Header {
            signed_header,
            validator_set,
            trusted_height: IbcHeight::new(0, 99).unwrap(),
            trusted_next_validator_set: trusted_set,
        }
    }

    #[test]
    fn full_header_serializes() {
        let header = make_full_header();
        let borsh = header_to_borsh(header);
        let bytes = borsh::to_vec(&borsh).unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn serialization_is_deterministic() {
        let bytes1 = borsh::to_vec(&header_to_borsh(make_full_header())).unwrap();
        let bytes2 = borsh::to_vec(&header_to_borsh(make_full_header())).unwrap();
        assert_eq!(bytes1, bytes2);
    }

    #[test]
    fn commit_sig_sorting_absent_comes_first() {
        let sigs = vec![
            make_commit_sig(5, "commit"),
            make_commit_sig(0, "absent"),
            make_commit_sig(1, "commit"),
        ];
        let borsh_commit = commit_to_borsh(make_commit(sigs));

        assert!(matches!(
            borsh_commit.signatures[0],
            BorshCommitSig::BlockIdFlagAbsent
        ));
    }

    #[test]
    fn commit_sig_sorting_by_address() {
        let sigs = vec![
            make_commit_sig(5, "commit"),
            make_commit_sig(1, "commit"),
            make_commit_sig(3, "commit"),
        ];
        let borsh_commit = commit_to_borsh(make_commit(sigs));

        let addresses: Vec<[u8; 20]> = borsh_commit
            .signatures
            .iter()
            .map(|s| match s {
                BorshCommitSig::BlockIdFlagCommit {
                    validator_address, ..
                } => *validator_address,
                _ => unreachable!(),
            })
            .collect();

        for pair in addresses.windows(2) {
            assert!(pair[0] <= pair[1]);
        }
    }

    #[test]
    fn commit_sig_sorting_mixed_commit_and_nil() {
        let sigs = vec![
            make_commit_sig(5, "nil"),
            make_commit_sig(0, "absent"),
            make_commit_sig(1, "commit"),
            make_commit_sig(0, "absent"),
            make_commit_sig(3, "nil"),
        ];
        let borsh_commit = commit_to_borsh(make_commit(sigs));

        let absent_count = borsh_commit
            .signatures
            .iter()
            .take_while(|s| matches!(s, BorshCommitSig::BlockIdFlagAbsent))
            .count();
        assert_eq!(absent_count, 2);

        let addresses: Vec<[u8; 20]> = borsh_commit.signatures[absent_count..]
            .iter()
            .map(|s| match s {
                BorshCommitSig::BlockIdFlagCommit {
                    validator_address, ..
                }
                | BorshCommitSig::BlockIdFlagNil {
                    validator_address, ..
                } => *validator_address,
                _ => unreachable!(),
            })
            .collect();

        for pair in addresses.windows(2) {
            assert!(pair[0] <= pair[1]);
        }
    }

    #[test]
    fn none_signature_becomes_zeroes() {
        let pk = PublicKey::from_raw_ed25519(&[42; 32]).unwrap();
        let sig = CommitSig::BlockIdFlagCommit {
            validator_address: AccountId::from(pk),
            timestamp: make_time(),
            signature: None,
        };
        match commit_sig_to_borsh(sig) {
            BorshCommitSig::BlockIdFlagCommit { signature, .. } => {
                assert_eq!(signature, [0u8; 64]);
            }
            _ => panic!("expected BlockIdFlagCommit"),
        }
    }

    #[test]
    fn header_with_no_optional_fields() {
        let v1 = make_validator(1, 10);
        let validator_set = ValidatorSet::new(vec![v1.clone()], Some(v1.clone()));

        let tm_header = TmHeader {
            version: Version { block: 11, app: 1 },
            chain_id: ChainId::try_from("test-1".to_string()).unwrap(),
            height: Height::try_from(1u64).unwrap(),
            time: make_time(),
            last_block_id: None,
            last_commit_hash: None,
            data_hash: None,
            validators_hash: make_hash(0x01),
            next_validators_hash: make_hash(0x02),
            consensus_hash: make_hash(0x03),
            app_hash: vec![0x04; 32].try_into().unwrap(),
            last_results_hash: None,
            evidence_hash: None,
            proposer_address: v1.address,
        };
        let commit = Commit {
            height: Height::try_from(1u64).unwrap(),
            round: Round::try_from(1u16).unwrap(),
            block_id: make_block_id(),
            signatures: vec![make_commit_sig(1, "commit")],
        };
        let signed_header = SignedHeader::new(tm_header, commit).unwrap();

        let header = Header {
            signed_header,
            validator_set: validator_set.clone(),
            trusted_height: IbcHeight::new(0, 1).unwrap(),
            trusted_next_validator_set: validator_set,
        };

        let borsh = header_to_borsh(header);
        assert!(borsh.signed_header.header.last_block_id.is_none());
        assert!(borsh.signed_header.header.last_commit_hash.is_none());
        assert!(borsh.signed_header.header.data_hash.is_none());
        assert!(borsh.signed_header.header.last_results_hash.is_none());
        assert!(borsh.signed_header.header.evidence_hash.is_none());

        borsh::to_vec(&borsh).unwrap();
    }
}
