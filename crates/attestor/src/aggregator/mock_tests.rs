// Aggregator tests against in-process attestors. Aggregated proofs are checked
// with `onchain_verify`, a port of `_verifySignaturesThreshold` from
// external/solidity-ibc-eureka/contracts/light-clients/attestation/AttestationLightClient.sol,
// so "the relayer built it" and "the light client accepts it" are judged separately.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alloy::primitives::{Address, B256, Signature, U256};
use alloy::signers::SignerSync;
use alloy::signers::local::PrivateKeySigner;
use sha2::{Digest, Sha256};
use tonic::Status;

use super::{AggregatedAttestation, Aggregator, AttestationKind, aggregate};
use crate::client::CommitmentType;
use crate::proto::attestor::Attestation;
use crate::test_support::{MockAttestor, dead_endpoint, wire};

// ATTESTATION_TYPE_STATE / ATTESTATION_TYPE_PACKET in the Solidity client.
const STATE_TAG: u8 = 0x01;
const PACKET_TAG: u8 = 0x02;
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
enum Revert {
    EmptySignatures,
    ThresholdNotMet,
    InvalidSignatureLength,
    SignatureInvalid,
    UnknownSigner(Address),
    DuplicateSigner(Address),
}

fn oracle_digest(data: &[u8], tag: u8) -> B256 {
    let mut preimage = vec![tag];
    preimage.extend_from_slice(&Sha256::digest(data));
    B256::from_slice(&Sha256::digest(&preimage))
}

// OpenZeppelin ECDSA.recover: 65 bytes, v in {27, 28}, low-s only.
fn oz_recover(digest: B256, sig: &[u8]) -> Result<Address, Revert> {
    if sig.len() != 65 {
        return Err(Revert::InvalidSignatureLength);
    }
    let half_n = U256::from_str_radix(
        "7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0",
        16,
    )
    .unwrap();
    let r = U256::from_be_slice(&sig[..32]);
    let s = U256::from_be_slice(&sig[32..64]);
    let v = sig[64];
    if s > half_n || !(v == 27 || v == 28) {
        return Err(Revert::SignatureInvalid);
    }
    Signature::new(r, s, v == 28)
        .recover_address_from_prehash(&digest)
        .map_err(|_| Revert::SignatureInvalid)
}

fn onchain_verify(
    data: &[u8],
    tag: u8,
    sigs: &[Vec<u8>],
    attestors: &[Address],
    min_required_sigs: usize,
) -> Result<(), Revert> {
    if sigs.is_empty() {
        return Err(Revert::EmptySignatures);
    }
    if sigs.len() < min_required_sigs {
        return Err(Revert::ThresholdNotMet);
    }
    let digest = oracle_digest(data, tag);
    let mut seen = Vec::new();
    for sig in sigs {
        let signer = oz_recover(digest, sig)?;
        if !attestors.contains(&signer) {
            return Err(Revert::UnknownSigner(signer));
        }
        if seen.contains(&signer) {
            return Err(Revert::DuplicateSigner(signer));
        }
        seen.push(signer);
    }
    Ok(())
}

fn keys(n: usize) -> Vec<PrivateKeySigner> {
    (0..n).map(|_| PrivateKeySigner::random()).collect()
}

fn addrs(keys: &[PrivateKeySigner]) -> Vec<Address> {
    keys.iter().map(PrivateKeySigner::address).collect()
}

fn sign(key: &PrivateKeySigner, data: &[u8], tag: u8) -> Vec<u8> {
    key.sign_hash_sync(&oracle_digest(data, tag))
        .unwrap()
        .as_bytes()
        .to_vec()
}

fn signed(key: &PrivateKeySigner, data: &[u8], tag: u8) -> Attestation {
    Attestation {
        height: 10,
        timestamp: Some(100),
        attested_data: data.to_vec(),
        signature: sign(key, data, tag),
    }
}

// An attestor that answers everything honestly, signing `data` with `key`.
fn honest(key: &PrivateKeySigner, height: u64, data: &[u8]) -> MockAttestor {
    attestor_with_tags(key, height, data, STATE_TAG, PACKET_TAG)
}

fn attestor_with_tags(
    key: &PrivateKeySigner,
    height: u64,
    data: &[u8],
    state_tag: u8,
    packet_tag: u8,
) -> MockAttestor {
    let (k1, d1) = (key.clone(), data.to_vec());
    let (k2, d2) = (key.clone(), data.to_vec());
    MockAttestor::default()
        .height(move || Ok(height))
        .state(move |h| {
            Ok(Some(wire::Attestation {
                height: h,
                timestamp: Some(1_000 + h),
                attested_data: d1.clone(),
                signature: sign(&k1, &d1, state_tag),
            }))
        })
        .packet(move |req| {
            Ok(Some(wire::Attestation {
                height: req.height,
                timestamp: None,
                attested_data: d2.clone(),
                signature: sign(&k2, &d2, packet_tag),
            }))
        })
}

fn failing() -> MockAttestor {
    MockAttestor::default()
        .height(|| Err(Status::unavailable("down")))
        .state(|_| Err(Status::unavailable("down")))
        .packet(|_| Err(Status::unavailable("down")))
}

async fn spawn_all(mocks: Vec<MockAttestor>) -> Vec<String> {
    let mut endpoints = Vec::new();
    for m in mocks {
        endpoints.push(m.spawn().await);
    }
    endpoints
}

fn aggregator(endpoints: &[String], signers: &[Address], quorum: usize) -> Aggregator {
    let addrs: Vec<String> = signers.iter().map(ToString::to_string).collect();
    Aggregator::new(endpoints, &addrs, quorum, TIMEOUT).unwrap()
}

fn signers_of(agg: &AggregatedAttestation, kind: AttestationKind) -> Vec<Address> {
    agg.signatures
        .iter()
        .map(|sig| {
            Signature::from_raw(sig)
                .unwrap()
                .recover_address_from_prehash(&super::signing_hash(&agg.attested_data, kind))
                .unwrap()
        })
        .collect()
}

#[test]
fn signing_hash_matches_solidity_tagged_signing_input() {
    for data in [&b""[..], b"x", &[0xff; 300]] {
        assert_eq!(
            super::signing_hash(data, AttestationKind::State),
            oracle_digest(data, STATE_TAG)
        );
        assert_eq!(
            super::signing_hash(data, AttestationKind::Packet),
            oracle_digest(data, PACKET_TAG)
        );
    }
}

#[tokio::test]
async fn state_attestation_from_honest_quorum_passes_onchain_verifier() {
    let ks = keys(3);
    let mocks: Vec<_> = ks.iter().map(|k| honest(k, 0, b"root")).collect();
    let logs: Vec<_> = mocks
        .iter()
        .map(|m| Arc::clone(&m.state_requests))
        .collect();
    let agg = aggregator(&spawn_all(mocks).await, &addrs(&ks), 2)
        .state_attestation(50)
        .await
        .unwrap();

    assert_eq!(agg.height, 50);
    assert_eq!(agg.timestamp, Some(1_050));
    assert_eq!(agg.attested_data, b"root");
    assert_eq!(agg.signatures.len(), 2);
    assert_eq!(
        onchain_verify(b"root", STATE_TAG, &agg.signatures, &addrs(&ks), 2),
        Ok(())
    );
    assert!(matches!(
        onchain_verify(b"root", PACKET_TAG, &agg.signatures, &addrs(&ks), 2),
        Err(Revert::UnknownSigner(_))
    ));
    for log in logs {
        assert_eq!(
            *log.lock().unwrap(),
            vec![wire::StateAttestationRequest { height: 50 }]
        );
    }
}

#[tokio::test]
async fn packet_attestation_passes_onchain_verifier_under_packet_tag() {
    let ks = keys(3);
    let mocks: Vec<_> = ks.iter().map(|k| honest(k, 0, b"commitments")).collect();
    let logs: Vec<_> = mocks
        .iter()
        .map(|m| Arc::clone(&m.packet_requests))
        .collect();
    let packets = vec![b"p1".to_vec(), b"p2".to_vec()];
    let agg = aggregator(&spawn_all(mocks).await, &addrs(&ks), 3)
        .packet_attestation(packets.clone(), 12, CommitmentType::Receipt)
        .await
        .unwrap();

    assert_eq!(agg.height, 12);
    assert_eq!(agg.timestamp, None);
    assert_eq!(
        onchain_verify(b"commitments", PACKET_TAG, &agg.signatures, &addrs(&ks), 3),
        Ok(())
    );
    for log in logs {
        assert_eq!(
            *log.lock().unwrap(),
            vec![wire::PacketAttestationRequest {
                packets: packets.clone(),
                height: 12,
                commitment_type: 2,
            }]
        );
    }
}

#[tokio::test]
async fn quorum_survives_unreachable_and_failing_attestors() {
    let ks = keys(4);
    let mut endpoints = spawn_all(vec![
        honest(&ks[0], 0, b"d"),
        failing(),
        honest(&ks[1], 0, b"d"),
    ])
    .await;
    endpoints.push(dead_endpoint().await);
    let agg = aggregator(&endpoints, &addrs(&ks), 2)
        .state_attestation(1)
        .await
        .unwrap();
    assert_eq!(
        onchain_verify(b"d", STATE_TAG, &agg.signatures, &addrs(&ks), 2),
        Ok(())
    );
}

#[tokio::test]
async fn state_attestation_below_quorum_errors_with_height_context() {
    let ks = keys(3);
    let mut endpoints = spawn_all(vec![honest(&ks[0], 0, b"d"), failing()]).await;
    endpoints.push(dead_endpoint().await);
    let err = aggregator(&endpoints, &addrs(&ks), 2)
        .state_attestation(7)
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("state attestation at height 7"), "{msg}");
    assert!(msg.contains("quorum not reached: 1 of 2"), "{msg}");
}

#[tokio::test]
async fn packet_attestation_with_no_responders_errors_with_commitment_context() {
    let ks = keys(2);
    let endpoints = spawn_all(vec![failing(), failing()]).await;
    let err = aggregator(&endpoints, &addrs(&ks), 1)
        .packet_attestation(vec![b"p".to_vec()], 3, CommitmentType::Ack)
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("Ack attestation at height 3"), "{msg}");
    assert!(msg.contains("no valid attestations"), "{msg}");
}

#[tokio::test]
async fn attestor_with_unconfigured_key_is_not_counted() {
    let ks = keys(2);
    let stranger = PrivateKeySigner::random();
    let endpoints = spawn_all(vec![honest(&ks[0], 0, b"d"), honest(&stranger, 0, b"d")]).await;

    assert!(
        aggregator(&endpoints, &addrs(&ks), 2)
            .state_attestation(1)
            .await
            .is_err()
    );
    let agg = aggregator(&endpoints, &addrs(&ks), 1)
        .state_attestation(1)
        .await
        .unwrap();
    assert_eq!(
        signers_of(&agg, AttestationKind::State),
        vec![ks[0].address()]
    );
    assert_eq!(
        onchain_verify(b"d", STATE_TAG, &agg.signatures, &addrs(&ks), 1),
        Ok(())
    );
}

#[tokio::test]
async fn packet_signature_under_state_tag_is_not_counted() {
    let ks = keys(2);
    let endpoints = spawn_all(vec![
        honest(&ks[0], 0, b"d"),
        attestor_with_tags(&ks[1], 0, b"d", STATE_TAG, STATE_TAG),
    ])
    .await;
    let err = aggregator(&endpoints, &addrs(&ks), 2)
        .packet_attestation(vec![b"p".to_vec()], 1, CommitmentType::Packet)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("1 of 2 valid signatures"));
}

#[tokio::test]
async fn two_endpoints_sharing_a_key_count_as_one_signer() {
    let ks = keys(2);
    let endpoints = spawn_all(vec![honest(&ks[0], 0, b"d"), honest(&ks[0], 0, b"d")]).await;
    let err = aggregator(&endpoints, &addrs(&ks), 2)
        .state_attestation(1)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("1 of 2 valid signatures"));

    // Why it must not count twice: the light client reverts on the pair.
    let sig = sign(&ks[0], b"d", STATE_TAG);
    assert_eq!(
        onchain_verify(b"d", STATE_TAG, &[sig.clone(), sig], &addrs(&ks), 2),
        Err(Revert::DuplicateSigner(ks[0].address()))
    );
}

#[tokio::test]
async fn byzantine_minority_on_other_data_does_not_block_quorum() {
    let ks = keys(3);
    let endpoints = spawn_all(vec![
        honest(&ks[0], 0, b"good"),
        honest(&ks[1], 0, b"evil"),
        honest(&ks[2], 0, b"good"),
    ])
    .await;
    let agg = aggregator(&endpoints, &addrs(&ks), 2)
        .state_attestation(1)
        .await
        .unwrap();
    assert_eq!(agg.attested_data, b"good");
    assert_eq!(
        onchain_verify(b"good", STATE_TAG, &agg.signatures, &addrs(&ks), 2),
        Ok(())
    );
}

#[tokio::test]
async fn equal_conflicting_quorums_error() {
    let ks = keys(4);
    let endpoints = spawn_all(vec![
        honest(&ks[0], 0, b"a"),
        honest(&ks[1], 0, b"b"),
        honest(&ks[2], 0, b"a"),
        honest(&ks[3], 0, b"b"),
    ])
    .await;
    let err = aggregator(&endpoints, &addrs(&ks), 2)
        .state_attestation(1)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("two attestations reached quorum"));
}

#[test]
fn larger_quorum_silently_wins_over_smaller_conflicting_quorum() {
    // Both groups would verify on-chain; only an exact tie is reported as a
    // disagreement. Pins current behaviour.
    let ks = keys(5);
    let mut atts: Vec<_> = ks[..3].iter().map(|k| signed(k, b"a", STATE_TAG)).collect();
    atts.extend(ks[3..].iter().map(|k| signed(k, b"b", STATE_TAG)));
    let b_sigs: Vec<_> = ks[3..].iter().map(|k| sign(k, b"b", STATE_TAG)).collect();
    assert_eq!(
        onchain_verify(b"b", STATE_TAG, &b_sigs, &addrs(&ks), 2),
        Ok(())
    );

    let agg = aggregate(
        atts,
        AttestationKind::State,
        &addrs(&ks).into_iter().collect(),
        2,
    )
    .unwrap();
    assert_eq!(agg.attested_data, b"a");
}

#[test]
fn equivocating_signer_counts_toward_each_group_it_signed() {
    let ks = keys(2);
    let atts = vec![
        signed(&ks[0], b"x", STATE_TAG),
        signed(&ks[0], b"y", STATE_TAG),
        signed(&ks[1], b"y", STATE_TAG),
    ];
    let agg = aggregate(
        atts,
        AttestationKind::State,
        &addrs(&ks).into_iter().collect(),
        2,
    )
    .unwrap();
    assert_eq!(agg.attested_data, b"y");
    assert_eq!(
        onchain_verify(b"y", STATE_TAG, &agg.signatures, &addrs(&ks), 2),
        Ok(())
    );
}

#[test]
fn signatures_are_ordered_by_signer_and_capped_at_quorum() {
    let ks = keys(5);
    let atts = ks.iter().map(|k| signed(k, b"d", STATE_TAG)).collect();
    let agg = aggregate(
        atts,
        AttestationKind::State,
        &addrs(&ks).into_iter().collect(),
        3,
    )
    .unwrap();
    let signers = signers_of(&agg, AttestationKind::State);
    assert_eq!(signers.len(), 3);
    assert!(signers.windows(2).all(|w| w[0] < w[1]));
    let mut expected = addrs(&ks);
    expected.sort();
    assert_eq!(signers, expected[..3]);
    assert_eq!(
        onchain_verify(b"d", STATE_TAG, &agg.signatures, &addrs(&ks), 3),
        Ok(())
    );
}

#[test]
fn unsigned_height_and_timestamp_come_from_first_group_member() {
    // Same as the reference relayer (packages/relayer/.../attestor_data.rs):
    // only attested_data is signed, so these fields are taken as reported.
    let ks = keys(2);
    let mut first = signed(&ks[0], b"d", STATE_TAG);
    first.height = 5;
    first.timestamp = Some(500);
    let mut second = signed(&ks[1], b"d", STATE_TAG);
    second.height = 6;
    second.timestamp = None;
    let agg = aggregate(
        vec![first, second],
        AttestationKind::State,
        &addrs(&ks).into_iter().collect(),
        2,
    )
    .unwrap();
    assert_eq!((agg.height, agg.timestamp), (5, Some(500)));
}

#[test]
fn no_attestations_is_an_error() {
    let err = aggregate(
        Vec::new(),
        AttestationKind::State,
        &addrs(&keys(1)).into_iter().collect(),
        1,
    )
    .unwrap_err();
    assert!(err.to_string().contains("no valid attestations"));
}

// Every attestor signs, but the one with the lowest address returns a
// signature the light client rejects. A correct aggregator drops it and still
// has an honest quorum; output is sorted by address, so if it's counted it
// lands in the proof.
fn proof_with_bad_lowest_signer(
    tamper: impl Fn(&[u8]) -> Vec<u8>,
) -> (Vec<Address>, Result<AggregatedAttestation, String>) {
    let mut ks = keys(3);
    ks.sort_by_key(PrivateKeySigner::address);
    let mut atts: Vec<_> = ks.iter().map(|k| signed(k, b"d", STATE_TAG)).collect();
    atts[0].signature = tamper(&atts[0].signature);
    assert_eq!(
        onchain_verify(
            b"d",
            STATE_TAG,
            std::slice::from_ref(&atts[0].signature),
            &addrs(&ks),
            1
        ),
        Err(Revert::SignatureInvalid)
    );
    let result = aggregate(
        atts,
        AttestationKind::State,
        &addrs(&ks).into_iter().collect(),
        2,
    )
    .map_err(|e| e.to_string());
    (addrs(&ks), result)
}

fn malleate(sig: &[u8]) -> Vec<u8> {
    let sig = Signature::from_raw(sig).unwrap();
    let n = U256::from_str_radix(
        "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141",
        16,
    )
    .unwrap();
    Signature::new(sig.r(), n - sig.s(), !sig.v())
        .as_bytes()
        .to_vec()
}

#[test]
fn high_s_signature_is_forwarded_as_low_s() {
    let (attestors, result) = proof_with_bad_lowest_signer(malleate);
    let agg = result.unwrap();
    assert_eq!(
        onchain_verify(b"d", STATE_TAG, &agg.signatures, &attestors, 2),
        Ok(())
    );
}

#[test]
fn eip155_style_v_is_forwarded_as_ethereum_v() {
    // Solidity accepts v in {27, 28}; the Solana program in {0, 1, 27, 28}.
    let (attestors, result) = proof_with_bad_lowest_signer(|sig| {
        let mut sig = sig.to_vec();
        sig[64] = sig[64] - 27 + 35;
        sig
    });
    let agg = result.unwrap();
    assert_eq!(
        onchain_verify(b"d", STATE_TAG, &agg.signatures, &attestors, 2),
        Ok(())
    );
}

#[test]
fn canonical_v_signature_is_forwarded_as_ethereum_v() {
    // The Solana attestation program accepts v in {0, 1}; the Solidity client
    // only {27, 28}, which every verifier accepts.
    let ks = keys(1);
    let mut att = signed(&ks[0], b"d", STATE_TAG);
    let ethereum_v = att.signature.clone();
    att.signature[64] -= 27;
    let agg = aggregate(
        vec![att],
        AttestationKind::State,
        &addrs(&ks).into_iter().collect(),
        1,
    )
    .unwrap();
    assert_eq!(agg.signatures, vec![ethereum_v]);
    assert_eq!(
        onchain_verify(b"d", STATE_TAG, &agg.signatures, &addrs(&ks), 1),
        Ok(())
    );
}

#[test]
fn into_proof_carries_data_and_signatures() {
    let agg = AggregatedAttestation {
        height: 1,
        timestamp: Some(2),
        attested_data: b"data".to_vec(),
        signatures: vec![vec![1], vec![2]],
    };
    let proof = agg.into_proof();
    assert_eq!(proof.attestation_data, b"data");
    assert_eq!(proof.signatures, vec![vec![1], vec![2]]);
}

#[tokio::test]
async fn latest_height_is_minimum_over_responders() {
    let ks = keys(3);
    let mut endpoints = spawn_all(vec![
        honest(&ks[0], 100, b""),
        honest(&ks[1], 90, b""),
        MockAttestor::default().garbage_latest_height(),
        honest(&ks[2], 95, b""),
    ])
    .await;
    endpoints.push(dead_endpoint().await);
    assert_eq!(
        aggregator(&endpoints, &addrs(&ks), 2)
            .latest_height()
            .await
            .unwrap(),
        90
    );
}

#[tokio::test]
async fn latest_height_below_quorum_errors() {
    let ks = keys(2);
    let endpoints = spawn_all(vec![honest(&ks[0], 100, b""), failing()]).await;
    let err = aggregator(&endpoints, &addrs(&ks), 2)
        .latest_height()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("1 of 2 responded"), "{err}");
}

#[tokio::test]
async fn wait_for_height_polls_attestors_until_target() {
    let ks = keys(1);
    let calls = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&calls);
    let mock = MockAttestor::default()
        .height(move || Ok(10 * (counter.fetch_add(1, Ordering::SeqCst) + 1)));
    let endpoints = spawn_all(vec![mock]).await;
    let reached = aggregator(&endpoints, &addrs(&ks), 1)
        .wait_for_height(25, TIMEOUT, Duration::from_millis(5))
        .await
        .unwrap();
    assert_eq!(reached, 30);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn wait_for_height_times_out_when_attestors_stall() {
    let ks = keys(1);
    let endpoints = spawn_all(vec![honest(&ks[0], 1, b"")]).await;
    let err = aggregator(&endpoints, &addrs(&ks), 1)
        .wait_for_height(5, Duration::from_millis(50), Duration::from_millis(10))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("did not finalize height 5"),
        "{err}"
    );
}

#[tokio::test]
async fn accessors_echo_configuration() {
    let ks = keys(2);
    let configured: Vec<String> = addrs(&ks).iter().map(ToString::to_string).collect();
    let endpoints = vec![dead_endpoint().await, dead_endpoint().await];
    let agg = Aggregator::new(&endpoints, &configured, 2, TIMEOUT).unwrap();
    assert_eq!(agg.attestor_addresses(), configured.as_slice());
    assert_eq!(agg.quorum(), 2);
}

#[tokio::test]
async fn new_rejects_quorum_above_endpoint_count_and_bad_endpoints() {
    let ks = keys(2);
    let configured: Vec<String> = addrs(&ks).iter().map(ToString::to_string).collect();
    let one = vec![dead_endpoint().await];
    let err = Aggregator::new(&one, &configured, 2, TIMEOUT).unwrap_err();
    assert!(
        err.to_string()
            .contains("exceeds the 2 addresses or 1 endpoints")
    );

    let bad = vec!["not a uri".to_string()];
    let err = Aggregator::new(&bad, &configured, 1, TIMEOUT).unwrap_err();
    assert!(err.to_string().contains("invalid attestor endpoint"));
}

#[tokio::test]
async fn lowercase_and_checksummed_addresses_are_equivalent() {
    let ks = keys(1);
    let endpoints = spawn_all(vec![honest(&ks[0], 0, b"d")]).await;
    let lower = format!("{:#x}", ks[0].address());
    let agg = Aggregator::new(&endpoints, &[lower], 1, TIMEOUT)
        .unwrap()
        .state_attestation(1)
        .await
        .unwrap();
    assert_eq!(agg.signatures.len(), 1);
}
