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
    #[must_use]
    pub fn new(endpoints: Vec<String>, quorum_threshold: usize) -> Self {
        Self {
            endpoints,
            quorum_threshold,
            http: reqwest::Client::new(),
        }
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

        let mut attested_data: Option<Vec<u8>> = None;
        let mut signatures = Vec::new();
        let mut timestamp: Option<u64> = None;

        let mut stream = futures::stream::iter(futs).buffer_unordered(endpoint_count);
        while let Some(result) = stream.next().await {
            let Some((ep, attestation)) = result else {
                continue;
            };

            if let Some(ref existing) = attested_data {
                if attestation.attested_data != *existing {
                    tracing::warn!(endpoint = %ep, "attestor returned different attested_data, skipping");
                    continue;
                }
            } else {
                attested_data = Some(attestation.attested_data);
                timestamp = attestation.timestamp;
            }

            signatures.extend(attestation.signatures);
        }

        let attested_data =
            attested_data.ok_or_else(|| eyre::eyre!("no attestor returned state data"))?;

        if signatures.len() < self.quorum_threshold {
            eyre::bail!(
                "attestor quorum not reached: got {} signatures, need {}",
                signatures.len(),
                self.quorum_threshold
            );
        }

        Ok(AggregatedAttestation {
            height,
            timestamp,
            attested_data,
            signatures,
        })
    }
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
}
