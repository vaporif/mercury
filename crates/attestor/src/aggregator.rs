use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use eyre::Result;
use futures::future::join_all;

use crate::client::{AttestorClient, CommitmentType};
use crate::proto::attestor::Attestation;
use crate::proto::lightclient::AttestationProof;

/// Recoverable secp256k1 signature: r (32) || s (32) || v (1).
const SIGNATURE_LEN: usize = 65;

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

/// Fans requests out to every attestor and returns once a quorum agrees.
#[derive(Clone, Debug)]
pub struct Aggregator {
    clients: Vec<AttestorClient>,
    quorum: usize,
}

impl Aggregator {
    pub fn new(endpoints: &[String], quorum: usize, timeout: Duration) -> Result<Self> {
        eyre::ensure!(quorum > 0, "attestor quorum must be at least 1");
        eyre::ensure!(
            endpoints.len() >= quorum,
            "attestor quorum {quorum} exceeds the {} configured endpoints",
            endpoints.len()
        );
        let clients = endpoints
            .iter()
            .map(|e| AttestorClient::new(e, timeout))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { clients, quorum })
    }

    /// Lowest finalized height reported by a quorum, so every member of the
    /// quorum can attest it.
    pub async fn latest_height(&self) -> Result<u64> {
        let heights = self.query_all(AttestorClient::latest_height).await;
        select_quorum_height(heights, self.quorum)
    }

    pub async fn state_attestation(&self, height: u64) -> Result<AggregatedAttestation> {
        let attestations = self.query_all(|c| c.state_attestation(height)).await;
        aggregate(attestations, self.quorum)
            .ok_or_else(|| eyre::eyre!("no attestor quorum for state at height {height}"))
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
        aggregate(attestations, self.quorum).ok_or_else(|| {
            eyre::eyre!("no attestor quorum for {commitment_type:?} attestation at height {height}")
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

/// Groups attestations by the data they sign and returns the first group
/// that reaches the quorum. Malformed signatures are dropped.
fn aggregate(attestations: Vec<Attestation>, quorum: usize) -> Option<AggregatedAttestation> {
    let mut groups: HashMap<Vec<u8>, Vec<Attestation>> = HashMap::new();
    for att in attestations {
        if att.signature.len() != SIGNATURE_LEN {
            tracing::warn!(
                len = att.signature.len(),
                "dropping attestation with bad signature length"
            );
            continue;
        }
        groups
            .entry(att.attested_data.clone())
            .or_default()
            .push(att);
    }
    let (attested_data, group) = groups.into_iter().find(|(_, g)| g.len() >= quorum)?;
    let first = group.first()?;
    Some(AggregatedAttestation {
        height: first.height,
        timestamp: first.timestamp,
        attested_data,
        signatures: group.into_iter().map(|a| a.signature).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(data: &[u8], sig_byte: u8) -> Attestation {
        Attestation {
            height: 10,
            timestamp: Some(100),
            attested_data: data.to_vec(),
            signature: vec![sig_byte; SIGNATURE_LEN],
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
    fn aggregate_collects_matching_signatures() {
        let agg = aggregate(vec![att(b"a", 1), att(b"a", 2)], 2);
        let agg = agg.as_ref();
        assert_eq!(agg.map(|a| a.signatures.len()), Some(2));
        assert_eq!(agg.map(|a| a.attested_data.as_slice()), Some(&b"a"[..]));
        assert_eq!(agg.and_then(|a| a.timestamp), Some(100));
    }

    #[test]
    fn aggregate_rejects_split_votes() {
        assert!(aggregate(vec![att(b"a", 1), att(b"b", 2)], 2).is_none());
    }

    #[test]
    fn aggregate_picks_majority_group() {
        let agg = aggregate(vec![att(b"a", 1), att(b"b", 2), att(b"b", 3)], 2);
        assert_eq!(agg.map(|a| a.attested_data), Some(b"b".to_vec()));
    }

    #[test]
    fn aggregate_drops_bad_signatures() {
        let mut bad = att(b"a", 1);
        bad.signature.truncate(64);
        assert!(aggregate(vec![bad, att(b"a", 2)], 2).is_none());
        assert!(aggregate(vec![att(b"a", 2)], 1).is_some());
    }
}
