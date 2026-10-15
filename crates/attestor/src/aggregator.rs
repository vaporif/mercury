use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::time::Duration;

use alloy_primitives::{Address, B256, Signature};
use eyre::Result;
use futures::future::join_all;
use sha2::{Digest, Sha256};

use crate::client::{AttestorClient, CommitmentType};
use crate::proto::attestor::Attestation;
use crate::proto::lightclient::AttestationProof;

/// Domain tags ibc-go's `attestations` client mixes into the signed hash so a
/// state attestation can't be replayed as a packet attestation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttestationKind {
    State = 0x01,
    Packet = 0x02,
}

/// Attestations from a quorum of attestors that agreed on the same data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AggregatedAttestation {
    pub height: u64,
    /// Block timestamp in seconds; only set for state attestations.
    pub timestamp: Option<u64>,
    pub attested_data: Vec<u8>,
    pub signatures: Vec<Vec<u8>>,
}

impl AggregatedAttestation {
    /// Proof bytes for ibc-go's `attestations` light client.
    #[must_use]
    pub fn into_proof(self) -> AttestationProof {
        AttestationProof {
            attestation_data: self.attested_data,
            signatures: self.signatures,
        }
    }
}

/// Fans requests out to every attestor and returns once a quorum of the
/// configured signers agrees.
#[derive(Clone, Debug)]
pub struct Aggregator {
    clients: Vec<AttestorClient>,
    signers: HashSet<Address>,
    attestor_addresses: Vec<String>,
    quorum: usize,
}

impl Aggregator {
    /// Must be called inside a Tokio runtime (channels connect lazily on it).
    pub fn new(
        endpoints: &[String],
        attestor_addresses: &[String],
        quorum: usize,
        timeout: Duration,
    ) -> Result<Self> {
        eyre::ensure!(!endpoints.is_empty(), "no attestor endpoints configured");
        eyre::ensure!(quorum > 0, "attestor quorum must be at least 1");
        let signers = attestor_addresses
            .iter()
            .map(|a| {
                a.parse::<Address>()
                    .map_err(|e| eyre::eyre!("invalid attestor address {a}: {e}"))
            })
            .collect::<Result<HashSet<_>>>()?;
        eyre::ensure!(
            signers.len() == attestor_addresses.len(),
            "duplicate attestor addresses"
        );
        eyre::ensure!(
            quorum <= signers.len() && quorum <= endpoints.len(),
            "attestor quorum {quorum} exceeds the {} addresses or {} endpoints",
            signers.len(),
            endpoints.len()
        );
        let clients = endpoints
            .iter()
            .map(|e| AttestorClient::new(e, timeout))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            clients,
            signers,
            attestor_addresses: attestor_addresses.to_vec(),
            quorum,
        })
    }

    #[must_use]
    pub fn attestor_addresses(&self) -> &[String] {
        &self.attestor_addresses
    }

    #[must_use]
    pub const fn quorum(&self) -> usize {
        self.quorum
    }

    /// Lowest finalized height reported by a quorum, so every member of the
    /// quorum can attest it.
    pub async fn latest_height(&self) -> Result<u64> {
        let heights = self.query_all(AttestorClient::latest_height).await;
        select_quorum_height(heights, self.quorum)
    }

    /// Waits until a quorum has finalized `target`, polling every `interval`.
    pub async fn wait_for_height(
        &self,
        target: u64,
        timeout: Duration,
        interval: Duration,
    ) -> Result<u64> {
        wait_for_height(|| self.latest_height(), target, timeout, interval).await
    }

    pub async fn state_attestation(&self, height: u64) -> Result<AggregatedAttestation> {
        let attestations = self.query_all(|c| c.state_attestation(height)).await;
        aggregate(
            attestations,
            AttestationKind::State,
            &self.signers,
            self.quorum,
        )
        .map_err(|e| e.wrap_err(format!("state attestation at height {height}")))
    }

    pub async fn packet_attestation(
        &self,
        packets: Vec<Vec<u8>>,
        height: u64,
        commitment_type: CommitmentType,
    ) -> Result<AggregatedAttestation> {
        let attestations = self
            .query_all(|c| c.packet_attestation(packets.clone(), height, commitment_type))
            .await;
        aggregate(
            attestations,
            AttestationKind::Packet,
            &self.signers,
            self.quorum,
        )
        .map_err(|e| {
            e.wrap_err(format!(
                "{commitment_type:?} attestation at height {height}"
            ))
        })
    }

    async fn query_all<'a, T, F, Fut>(&'a self, f: F) -> Vec<T>
    where
        F: Fn(&'a AttestorClient) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        join_all(self.clients.iter().map(|c| {
            let fut = f(c);
            async move {
                fut.await
                    .inspect_err(|e| {
                        tracing::warn!(endpoint = c.endpoint(), error = %e, "attestor request failed");
                    })
                    .ok()
            }
        }))
        .await
        .into_iter()
        .flatten()
        .collect()
    }
}

/// Polls `latest` until it reaches `target`. Returns the height it reached.
pub async fn wait_for_height<F, Fut>(
    latest: F,
    target: u64,
    timeout: Duration,
    interval: Duration,
) -> Result<u64>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<u64>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match latest().await {
            Ok(height) if height >= target => return Ok(height),
            Ok(height) => tracing::debug!(height, target, "waiting for attestors to finalize"),
            Err(e) => tracing::debug!(error = %e, target, "attestor height unavailable"),
        }
        if tokio::time::Instant::now() + interval > deadline {
            eyre::bail!("attestors did not finalize height {target} within {timeout:?}");
        }
        tokio::time::sleep(interval).await;
    }
}

fn select_quorum_height(heights: Vec<u64>, quorum: usize) -> Result<u64> {
    eyre::ensure!(
        heights.len() >= quorum,
        "attestor quorum not reached: {} of {quorum} responded",
        heights.len()
    );
    heights
        .into_iter()
        .min()
        .ok_or_else(|| eyre::eyre!("no attestor responded"))
}

type SignaturesBySigner = HashMap<Address, Vec<u8>>;

/// The prehash ibc-go verifies: `sha256(kind || sha256(data))`.
fn signing_hash(data: &[u8], kind: AttestationKind) -> B256 {
    let inner = Sha256::digest(data);
    let mut hasher = Sha256::new();
    hasher.update([kind as u8]);
    hasher.update(inner);
    B256::from_slice(&hasher.finalize())
}

fn recover_signer(att: &Attestation, kind: AttestationKind) -> Result<Address> {
    let sig = Signature::from_raw(&att.signature)?;
    Ok(sig.recover_address_from_prehash(&signing_hash(&att.attested_data, kind))?)
}

/// Groups attestations by the data they sign, keeping one signature per
/// configured signer, and returns `quorum` signatures from the largest group.
///
/// ibc-go rejects a proof outright if any signature is from an unknown or
/// repeated signer, so those are dropped here rather than counted.
fn aggregate(
    attestations: Vec<Attestation>,
    kind: AttestationKind,
    signers: &HashSet<Address>,
    quorum: usize,
) -> Result<AggregatedAttestation> {
    let mut groups: HashMap<Vec<u8>, (Attestation, SignaturesBySigner)> = HashMap::new();
    for att in attestations {
        let signer = match recover_signer(&att, kind) {
            Ok(signer) if signers.contains(&signer) => signer,
            Ok(signer) => {
                tracing::warn!(%signer, "dropping attestation from unknown signer");
                continue;
            }
            Err(e) => {
                tracing::warn!(error = %e, "dropping attestation with invalid signature");
                continue;
            }
        };
        let (_, sigs) = groups
            .entry(att.attested_data.clone())
            .or_insert_with(|| (att.clone(), HashMap::new()));
        sigs.entry(signer).or_insert(att.signature);
    }

    let mut ranked: Vec<_> = groups.into_values().collect();
    ranked.sort_by_key(|(_, sigs)| std::cmp::Reverse(sigs.len()));
    let (first, sigs) = match ranked.as_slice() {
        [] => eyre::bail!("no valid attestations"),
        [(_, best), (_, second), ..] if best.len() == second.len() && second.len() >= quorum => {
            eyre::bail!("attestors disagree: two attestations reached quorum")
        }
        [(first, sigs), ..] => (first, sigs),
    };
    eyre::ensure!(
        sigs.len() >= quorum,
        "attestor quorum not reached: {} of {quorum} valid signatures",
        sigs.len()
    );

    let mut signed: Vec<_> = sigs.iter().collect();
    signed.sort_by_key(|(addr, _)| **addr);
    Ok(AggregatedAttestation {
        height: first.height,
        timestamp: first.timestamp,
        attested_data: first.attested_data.clone(),
        signatures: signed
            .into_iter()
            .take(quorum)
            .map(|(_, sig)| sig.clone())
            .collect(),
    })
}

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "mock attestor handlers return tonic::Status"
)]
mod mock_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::SignerSync;
    use alloy::signers::local::PrivateKeySigner;

    fn signers(n: usize) -> Vec<PrivateKeySigner> {
        (0..n).map(|_| PrivateKeySigner::random()).collect()
    }

    fn set(keys: &[PrivateKeySigner]) -> HashSet<Address> {
        keys.iter().map(PrivateKeySigner::address).collect()
    }

    fn att(key: &PrivateKeySigner, data: &[u8], kind: AttestationKind) -> Attestation {
        let sig = key
            .sign_hash_sync(&signing_hash(data, kind))
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default();
        Attestation {
            height: 10,
            timestamp: Some(100),
            attested_data: data.to_vec(),
            signature: sig,
        }
    }

    #[test]
    fn quorum_height_is_minimum() {
        assert_eq!(select_quorum_height(vec![12, 10, 11], 2).ok(), Some(10));
    }

    #[test]
    fn quorum_height_fails_below_quorum() {
        assert!(select_quorum_height(vec![12], 2).is_err());
        assert!(select_quorum_height(vec![], 1).is_err());
    }

    #[test]
    fn aggregates_quorum_of_known_signers() {
        let keys = signers(3);
        let atts = keys
            .iter()
            .map(|k| att(k, b"a", AttestationKind::State))
            .collect();
        let agg = aggregate(atts, AttestationKind::State, &set(&keys), 2).ok();
        assert_eq!(agg.as_ref().map(|a| a.signatures.len()), Some(2));
        assert_eq!(agg.as_ref().and_then(|a| a.timestamp), Some(100));
        assert_eq!(agg.map(|a| a.attested_data), Some(b"a".to_vec()));
    }

    #[test]
    fn duplicate_signer_counts_once() {
        let keys = signers(2);
        let dup = att(&keys[0], b"a", AttestationKind::State);
        let result = aggregate(
            vec![dup.clone(), dup],
            AttestationKind::State,
            &set(&keys),
            2,
        );
        assert!(result.is_err());
    }

    #[test]
    fn unknown_signer_is_dropped() {
        let keys = signers(2);
        let stranger = PrivateKeySigner::random();
        let atts = vec![
            att(&keys[0], b"a", AttestationKind::State),
            att(&stranger, b"a", AttestationKind::State),
        ];
        assert!(aggregate(atts.clone(), AttestationKind::State, &set(&keys), 2).is_err());
        let agg = aggregate(atts, AttestationKind::State, &set(&keys), 1).ok();
        assert_eq!(agg.map(|a| a.signatures.len()), Some(1));
    }

    #[test]
    fn wrong_domain_tag_is_rejected() {
        let keys = signers(1);
        let atts = vec![att(&keys[0], b"a", AttestationKind::State)];
        assert!(aggregate(atts, AttestationKind::Packet, &set(&keys), 1).is_err());
    }

    #[test]
    fn malformed_signature_is_dropped() {
        let keys = signers(1);
        let mut bad = att(&keys[0], b"a", AttestationKind::State);
        bad.signature.truncate(64);
        assert!(aggregate(vec![bad], AttestationKind::State, &set(&keys), 1).is_err());
    }

    #[test]
    fn largest_group_wins() {
        let keys = signers(3);
        let atts = vec![
            att(&keys[0], b"a", AttestationKind::State),
            att(&keys[1], b"b", AttestationKind::State),
            att(&keys[2], b"b", AttestationKind::State),
        ];
        let agg = aggregate(atts, AttestationKind::State, &set(&keys), 1).ok();
        assert_eq!(agg.map(|a| a.attested_data), Some(b"b".to_vec()));
    }

    #[test]
    fn conflicting_quorums_error() {
        let keys = signers(4);
        let atts = vec![
            att(&keys[0], b"a", AttestationKind::State),
            att(&keys[1], b"a", AttestationKind::State),
            att(&keys[2], b"b", AttestationKind::State),
            att(&keys[3], b"b", AttestationKind::State),
        ];
        assert!(aggregate(atts, AttestationKind::State, &set(&keys), 2).is_err());
    }

    #[test]
    fn split_vote_below_quorum_errors() {
        let keys = signers(2);
        let atts = vec![
            att(&keys[0], b"a", AttestationKind::State),
            att(&keys[1], b"b", AttestationKind::State),
        ];
        assert!(aggregate(atts, AttestationKind::State, &set(&keys), 2).is_err());
    }

    #[tokio::test]
    async fn rejects_bad_config() {
        let endpoints = vec!["http://127.0.0.1:1".to_string()];
        let addr = PrivateKeySigner::random().address().to_string();
        let t = Duration::from_secs(1);
        assert!(Aggregator::new(&endpoints, std::slice::from_ref(&addr), 1, t).is_ok());
        assert!(Aggregator::new(&endpoints, std::slice::from_ref(&addr), 0, t).is_err());
        assert!(Aggregator::new(&endpoints, std::slice::from_ref(&addr), 2, t).is_err());
        assert!(Aggregator::new(&endpoints, &[addr.clone(), addr], 1, t).is_err());
        assert!(Aggregator::new(&endpoints, &["nope".to_string()], 1, t).is_err());
        assert!(Aggregator::new(&[], &["nope".to_string()], 1, t).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_height_returns_once_reached() {
        let calls = std::sync::atomic::AtomicU64::new(0);
        let reached = wait_for_height(
            || {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move { Ok(5 + n * 5) }
            },
            15,
            Duration::from_secs(10),
            Duration::from_secs(1),
        )
        .await
        .ok();
        assert_eq!(reached, Some(15));
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_height_survives_errors_then_times_out() {
        let result = wait_for_height(
            || async { Err::<u64, _>(eyre::eyre!("down")) },
            1,
            Duration::from_secs(3),
            Duration::from_secs(1),
        )
        .await;
        assert!(result.is_err());
    }
}
