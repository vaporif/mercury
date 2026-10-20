use std::collections::{HashMap, HashSet};

use futures::stream::StreamExt;
use mercury_core::error::Result;
use serde::Deserialize;

#[derive(Clone, Debug)]
pub struct AggregatorClient {
    endpoints: Vec<String>,
    quorum_threshold: usize,
    http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
pub struct AggregatedAttestation {
    pub height: u64,
    pub timestamp: Option<u64>,
    #[serde(with = "base64_bytes")]
    pub attested_data: Vec<u8>,
    #[serde(with = "base64_bytes_vec")]
    pub signatures: Vec<Vec<u8>>,
}

impl AggregatorClient {
    pub fn new(endpoints: Vec<String>, quorum_threshold: usize) -> Result<Self> {
        eyre::ensure!(
            (1..=endpoints.len()).contains(&quorum_threshold),
            "attestor quorum_threshold must be 1..={}",
            endpoints.len()
        );
        Ok(Self {
            endpoints,
            quorum_threshold,
            http: reqwest::Client::new(),
        })
    }

    pub async fn get_latest_height(&self) -> Result<u64> {
        let endpoint_count = self.endpoints.len();
        let futs: Vec<_> = self
            .endpoints
            .iter()
            .map(|endpoint| {
                let url = format!("{endpoint}/latest_height");
                let http = self.http.clone();
                let ep = endpoint.clone();
                async move {
                    http.get(&url).send().await
                        .inspect_err(|e| tracing::warn!(endpoint = %ep, error = %e, "attestor unreachable"))
                        .ok()?
                        .json::<u64>().await
                        .inspect_err(|e| tracing::warn!(endpoint = %ep, error = %e, "failed to decode height"))
                        .ok()
                }
            })
            .collect();
        let mut heights: Vec<u64> = futures::stream::iter(futs)
            .buffer_unordered(endpoint_count)
            .filter_map(std::future::ready)
            .collect()
            .await;

        if heights.len() < self.quorum_threshold {
            eyre::bail!(
                "attestor quorum not reached: got {}, need {}",
                heights.len(),
                self.quorum_threshold
            );
        }
        heights.sort_unstable();
        Ok(heights[heights.len() - self.quorum_threshold])
    }

    pub async fn get_state_attestation(&self, height: u64) -> Result<AggregatedAttestation> {
        let endpoint_count = self.endpoints.len();
        let futs: Vec<_> = self
            .endpoints
            .iter()
            .map(|endpoint| {
                let url = format!("{endpoint}/state_attestation/{height}");
                let http = self.http.clone();
                let ep = endpoint.clone();
                async move {
                    let resp = http.get(&url).send().await
                        .inspect_err(|e| tracing::warn!(endpoint = %ep, error = %e, "attestor unreachable"))
                        .ok()?;
                    let attestation: AggregatedAttestation = resp.json().await
                        .inspect_err(|e| tracing::warn!(endpoint = %ep, error = %e, "failed to decode attestor response"))
                        .ok()?;
                    Some((ep, attestation))
                }
            })
            .collect();

        let responses: Vec<_> = futures::stream::iter(futs)
            .buffer_unordered(endpoint_count)
            .filter_map(std::future::ready)
            .collect()
            .await;

        select_quorum_attestation(responses, height, self.quorum_threshold)
    }
}

#[derive(Default)]
struct StateGroup {
    attestors: HashSet<String>,
    signatures: Vec<Vec<u8>>,
}

impl StateGroup {
    /// Neither extra signatures from one attestor nor repeated signatures
    /// from several count towards quorum.
    fn support(&self) -> usize {
        self.attestors.len().min(self.signatures.len())
    }
}

/// Groups responses by the state they attest and picks the one backed by a
/// quorum, so a fast attestor reporting different data can't crowd out an
/// honest quorum. Signatures are deduplicated because the light client
/// rejects proofs with a repeated signer.
fn select_quorum_attestation(
    responses: Vec<(String, AggregatedAttestation)>,
    height: u64,
    quorum: usize,
) -> Result<AggregatedAttestation> {
    let mut groups: HashMap<(Vec<u8>, Option<u64>), StateGroup> = HashMap::new();
    for (endpoint, attestation) in responses {
        let group = groups
            .entry((attestation.attested_data, attestation.timestamp))
            .or_default();
        group.attestors.insert(endpoint);
        for sig in attestation.signatures {
            if !group.signatures.contains(&sig) {
                group.signatures.push(sig);
            }
        }
    }

    let mut ranked: Vec<_> = groups.into_iter().collect();
    ranked.sort_by_key(|(_, group)| std::cmp::Reverse(group.support()));
    if ranked.get(1).is_some_and(|(_, g)| g.support() >= quorum) {
        eyre::bail!("attestors disagree: conflicting states reached quorum");
    }
    let Some(((attested_data, timestamp), group)) = ranked.into_iter().next() else {
        eyre::bail!("no attestor returned state data");
    };
    let got = group.support();
    if got < quorum {
        eyre::bail!("attestor quorum not reached: got {got} attestors, need {quorum}");
    }

    Ok(AggregatedAttestation {
        height,
        timestamp,
        attested_data,
        signatures: group.signatures,
    })
}

mod base64_bytes {
    use base64::Engine;
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(&s)
            .map_err(serde::de::Error::custom)
    }
}

mod base64_bytes_vec {
    use base64::Engine;
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<u8>>, D::Error> {
        let strings: Vec<String> = Vec::deserialize(d)?;
        strings
            .into_iter()
            .map(|s| {
                base64::engine::general_purpose::STANDARD
                    .decode(&s)
                    .map_err(serde::de::Error::custom)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_state_attestation() {
        let json = r#"{
            "height": 100,
            "timestamp": 1234567890,
            "attested_data": "AQID",
            "signatures": ["BAUG", "BwgJ"]
        }"#;
        let attestation: AggregatedAttestation = serde_json::from_str(json).unwrap();
        assert_eq!(attestation.height, 100);
        assert_eq!(attestation.timestamp, Some(1_234_567_890));
        assert_eq!(attestation.attested_data, vec![1, 2, 3]);
        assert_eq!(attestation.signatures.len(), 2);
    }

    use crate::chain::test_support::spawn_http;

    const UNREACHABLE: &str = "http://127.0.0.1:1";

    /// Attestor serving a fixed height and a fixed attestation for any height.
    async fn attestor(height: &str, attestation: &str) -> String {
        let height = height.to_string();
        let attestation = attestation.to_string();
        spawn_http(move |req| {
            assert_eq!(req.method, "GET");
            if req.path == "/latest_height" {
                (200, height.clone())
            } else if req.path.starts_with("/state_attestation/") {
                (200, attestation.clone())
            } else {
                (404, String::new())
            }
        })
        .await
    }

    fn attestation_json(data_b64: &str, sigs: &[&str], timestamp: u64) -> String {
        serde_json::json!({
            "height": 1,
            "timestamp": timestamp,
            "attested_data": data_b64,
            "signatures": sigs,
        })
        .to_string()
    }

    #[test]
    fn deserialize_rejects_invalid_base64() {
        let bad_data = r#"{"height":1,"timestamp":null,"attested_data":"!!","signatures":[]}"#;
        assert!(serde_json::from_str::<AggregatedAttestation>(bad_data).is_err());
        let bad_sig = r#"{"height":1,"timestamp":null,"attested_data":"AQ==","signatures":["%%"]}"#;
        assert!(serde_json::from_str::<AggregatedAttestation>(bad_sig).is_err());
    }

    #[test]
    fn deserialize_allows_missing_timestamp() {
        let json = r#"{"height":1,"timestamp":null,"attested_data":"","signatures":[]}"#;
        let a: AggregatedAttestation = serde_json::from_str(json).unwrap();
        assert_eq!(a.timestamp, None);
        assert_eq!(a.attested_data, Vec::<u8>::new());
    }

    #[tokio::test]
    async fn latest_height_picks_highest_height_reached_by_quorum() {
        let endpoints = vec![
            attestor("10", "").await,
            attestor("30", "").await,
            attestor("20", "").await,
        ];
        let height = AggregatorClient::new(endpoints.clone(), 2)
            .unwrap()
            .get_latest_height()
            .await
            .unwrap();
        assert_eq!(height, 20);

        let height = AggregatorClient::new(endpoints.clone(), 3)
            .unwrap()
            .get_latest_height()
            .await
            .unwrap();
        assert_eq!(height, 10);

        let height = AggregatorClient::new(endpoints, 1)
            .unwrap()
            .get_latest_height()
            .await
            .unwrap();
        assert_eq!(height, 30);
    }

    #[tokio::test]
    async fn latest_height_skips_unreachable_and_malformed() {
        let endpoints = vec![
            attestor("15", "").await,
            attestor("not-a-number", "").await,
            UNREACHABLE.to_string(),
        ];
        let client = AggregatorClient::new(endpoints.clone(), 1).unwrap();
        assert_eq!(client.get_latest_height().await.unwrap(), 15);

        let err = AggregatorClient::new(endpoints, 2)
            .unwrap()
            .get_latest_height()
            .await
            .unwrap_err();
        assert!(err.to_string().contains("got 1, need 2"), "{err}");
    }

    #[tokio::test]
    async fn state_attestation_collects_signatures_from_agreeing_attestors() {
        let endpoints = vec![
            attestor("0", &attestation_json("AQID", &["BAUG"], 77)).await,
            attestor("0", &attestation_json("AQID", &["BwgJ"], 77)).await,
        ];
        let a = AggregatorClient::new(endpoints, 2)
            .unwrap()
            .get_state_attestation(123)
            .await
            .unwrap();
        assert_eq!(a.height, 123);
        assert_eq!(a.timestamp, Some(77));
        assert_eq!(a.attested_data, vec![1, 2, 3]);
        let mut sigs = a.signatures;
        sigs.sort();
        assert_eq!(sigs, vec![vec![4, 5, 6], vec![7, 8, 9]]);
    }

    #[tokio::test]
    async fn state_attestation_requests_given_height() {
        let url = spawn_http(|req| {
            assert_eq!(req.path, "/state_attestation/4242");
            (200, attestation_json("AQ==", &["Ag=="], 1))
        })
        .await;
        let a = AggregatorClient::new(vec![url], 1)
            .unwrap()
            .get_state_attestation(4242)
            .await
            .unwrap();
        assert_eq!(a.signatures, vec![vec![2]]);
    }

    #[tokio::test]
    async fn state_attestation_drops_conflicting_data() {
        let endpoints = vec![
            attestor("0", &attestation_json("AQID", &["BAUG"], 1)).await,
            attestor("0", &attestation_json("CQkJ", &["BwgJ"], 1)).await,
        ];
        let err = AggregatorClient::new(endpoints, 2)
            .unwrap()
            .get_state_attestation(1)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("got 1 attestors, need 2"), "{err}");
    }

    #[tokio::test]
    async fn state_attestation_fails_when_nobody_answers() {
        let endpoints = vec![
            UNREACHABLE.to_string(),
            attestor("0", "{\"garbage\":true}").await,
        ];
        let err = AggregatorClient::new(endpoints, 1)
            .unwrap()
            .get_state_attestation(1)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("no attestor returned state data"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn state_attestation_skips_unreachable_attestor() {
        let endpoints = vec![
            UNREACHABLE.to_string(),
            attestor("0", &attestation_json("AQ==", &["Ag=="], 1)).await,
        ];
        let a = AggregatorClient::new(endpoints, 1)
            .unwrap()
            .get_state_attestation(1)
            .await
            .unwrap();
        assert_eq!(a.signatures.len(), 1);
    }

    /// Serves `body` for every request after `delay`, to control response order.
    async fn delayed_attestor(delay: std::time::Duration, body: String) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let body = body.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    tokio::time::sleep(delay).await;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn state_attestation_quorum_counts_attestors_not_signatures() {
        let endpoints = vec![
            UNREACHABLE.to_string(),
            attestor("0", &attestation_json("AQ==", &["Ag==", "Aw=="], 1)).await,
        ];
        let err = AggregatorClient::new(endpoints, 2)
            .unwrap()
            .get_state_attestation(1)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("quorum not reached"), "{err}");
    }

    #[tokio::test]
    async fn state_attestation_dedupes_repeated_signatures() {
        let endpoints = vec![
            attestor("0", &attestation_json("AQ==", &["Ag=="], 1)).await,
            attestor("0", &attestation_json("AQ==", &["Ag=="], 1)).await,
        ];
        let client = AggregatorClient::new(endpoints.clone(), 1).unwrap();
        assert_eq!(
            client.get_state_attestation(1).await.unwrap().signatures,
            vec![vec![2]]
        );
        let err = AggregatorClient::new(endpoints, 2)
            .unwrap()
            .get_state_attestation(1)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("quorum not reached"), "{err}");
    }

    #[tokio::test]
    async fn state_attestation_honest_quorum_beats_fast_conflicting_attestor() {
        let slow = std::time::Duration::from_millis(200);
        let endpoints = vec![
            attestor("0", &attestation_json("CQkJ", &["AQ=="], 9)).await,
            delayed_attestor(slow, attestation_json("AQID", &["Ag=="], 1)).await,
            delayed_attestor(slow, attestation_json("AQID", &["Aw=="], 1)).await,
        ];
        let a = AggregatorClient::new(endpoints, 2)
            .unwrap()
            .get_state_attestation(1)
            .await
            .unwrap();
        assert_eq!(a.attested_data, vec![1, 2, 3]);
        assert_eq!(a.timestamp, Some(1));
        let mut sigs = a.signatures;
        sigs.sort();
        assert_eq!(sigs, vec![vec![2], vec![3]]);
    }

    #[tokio::test]
    async fn state_attestation_rejects_two_conflicting_quorums() {
        let endpoints = vec![
            attestor("0", &attestation_json("AQID", &["AQ=="], 1)).await,
            attestor("0", &attestation_json("CQkJ", &["Ag=="], 1)).await,
        ];
        let err = AggregatorClient::new(endpoints, 1)
            .unwrap()
            .get_state_attestation(1)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("disagree"), "{err}");
    }

    #[test]
    fn new_rejects_empty_endpoints_and_bad_quorum() {
        assert!(AggregatorClient::new(vec![], 1).is_err());
        assert!(AggregatorClient::new(vec![], 0).is_err());
        assert!(AggregatorClient::new(vec![UNREACHABLE.to_string()], 0).is_err());
        assert!(AggregatorClient::new(vec![UNREACHABLE.to_string()], 2).is_err());
        assert!(AggregatorClient::new(vec![UNREACHABLE.to_string()], 1).is_ok());
    }
}
