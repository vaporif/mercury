use eyre::Context;

use mercury_chain_traits::queries::ChainStatusQuery;

use crate::aggregator::AggregatorClient;
use crate::builders::{CreateClientPayload, UpdateClientPayload};
use crate::chain::EthereumChain;
use crate::config::ClientPayloadMode;

impl EthereumChain {
    pub(crate) async fn build_create_client_payload_attested(
        &self,
    ) -> mercury_core::error::Result<CreateClientPayload> {
        let status = self.query_chain_status().await?;
        let height = status.height.0;
        let timestamp = status.timestamp.0;

        let ClientPayloadMode::Attested {
            attestor_addresses,
            quorum_threshold,
            ..
        } = &self.config.client_payload_mode
        else {
            unreachable!("attested path called in non-attested mode")
        };

        let client_state = serde_json::json!({
            "height": height,
            "timestamp": timestamp,
            "attestor_addresses": attestor_addresses,
            "min_required_sigs": quorum_threshold,
        });

        let client_state_bytes =
            serde_json::to_vec(&client_state).wrap_err("serializing attested client state")?;

        Ok(CreateClientPayload {
            client_state: client_state_bytes,
            consensus_state: vec![],
            counterparty_client_id: None,
            counterparty_merkle_prefix: None,
        })
    }

    pub(crate) async fn build_update_client_payload_attested(
        &self,
        aggregator: &AggregatorClient,
    ) -> mercury_core::error::Result<UpdateClientPayload> {
        let height = aggregator
            .get_latest_height()
            .await
            .wrap_err("aggregator: getting latest height")?;

        let attestation = aggregator
            .get_state_attestation(height)
            .await
            .wrap_err("aggregator: getting state attestation")?;

        let proof = serde_json::json!({
            "attested_data": attestation.attested_data,
            "signatures": attestation.signatures,
            "height": attestation.height,
            "timestamp": attestation.timestamp,
        });

        let proof_bytes = serde_json::to_vec(&proof).wrap_err("serializing attestation proof")?;

        Ok(UpdateClientPayload {
            headers: vec![proof_bytes],
            target_execution_height: None,
            target_slot: None,
            required_dst_timestamp_secs: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::chain::PayloadClient;
    use crate::chain::test_support::{chain_from_config, spawn_http, spawn_rpc, test_config};

    fn attested_chain(rpc: &str, endpoints: Vec<String>, quorum: usize) -> EthereumChain {
        let attestor_addresses = (1..=endpoints.len())
            .map(|i| format!("0x{i:040x}"))
            .collect();
        let mode = ClientPayloadMode::Attested {
            attestor_endpoints: endpoints.clone(),
            attestor_addresses,
            quorum_threshold: quorum,
        };
        chain_from_config(
            test_config(rpc, mode),
            PayloadClient::Attested(AggregatorClient::new(endpoints, quorum).unwrap()),
        )
    }

    #[tokio::test]
    async fn create_payload_snapshots_chain_status_and_attestors() {
        let rpc = spawn_rpc(|method, _| match method {
            "eth_blockNumber" => Ok(json!("0x64")),
            "eth_getBlockByNumber" => {
                let mut block =
                    alloy::rpc::types::Block::<alloy::rpc::types::Transaction>::default();
                block.header.inner.number = 100;
                block.header.inner.timestamp = 1_700_000_000;
                Ok(serde_json::to_value(block).unwrap())
            }
            other => Err(format!("unexpected {other}")),
        })
        .await;
        let endpoints = vec!["http://a1".to_string(), "http://a2".to_string()];
        let chain = attested_chain(&rpc, endpoints, 2);

        let payload = chain.build_create_client_payload_attested().await.unwrap();
        let cs: Value = serde_json::from_slice(&payload.client_state).unwrap();
        assert_eq!(
            cs,
            json!({
                "height": 100,
                "timestamp": 1_700_000_000,
                "attestor_addresses": [
                    "0x0000000000000000000000000000000000000001",
                    "0x0000000000000000000000000000000000000002",
                ],
                "min_required_sigs": 2,
            })
        );
        assert_eq!(payload.consensus_state, Vec::<u8>::new());
    }

    // Attestation light clients (ibc-go attestations.v1, Solidity, Solana)
    // check recovered ECDSA signers against `attestor_addresses`, so every
    // entry must be an Ethereum address, not an attestor RPC endpoint.
    #[tokio::test]
    async fn create_payload_attestor_addresses_are_ethereum_addresses() {
        let rpc = spawn_rpc(|method, _| match method {
            "eth_blockNumber" => Ok(json!("0x64")),
            "eth_getBlockByNumber" => Ok(serde_json::to_value(alloy::rpc::types::Block::<
                alloy::rpc::types::Transaction,
            >::default())
            .unwrap()),
            other => Err(format!("unexpected {other}")),
        })
        .await;
        let chain = attested_chain(&rpc, vec!["http://attestor-0:8080".to_string()], 1);

        let payload = chain.build_create_client_payload_attested().await.unwrap();
        let cs: Value = serde_json::from_slice(&payload.client_state).unwrap();
        for entry in cs["attestor_addresses"].as_array().unwrap() {
            let entry = entry.as_str().unwrap();
            assert!(
                entry.parse::<alloy::primitives::Address>().is_ok(),
                "attestor_addresses entry {entry:?} is not an Ethereum address"
            );
        }
    }

    #[tokio::test]
    async fn create_payload_propagates_status_failure() {
        let rpc = spawn_rpc(|_, _| Err("down".to_string())).await;
        let chain = attested_chain(&rpc, vec!["http://a".to_string()], 1);
        assert!(chain.build_create_client_payload_attested().await.is_err());
    }

    #[tokio::test]
    async fn update_payload_wraps_latest_attestation() {
        let attestor = spawn_http(|req| match req.path.as_str() {
            "/latest_height" => (200, "55".to_string()),
            "/state_attestation/55" => (
                200,
                json!({
                    "height": 55,
                    "timestamp": 9,
                    "attested_data": "AQI=",
                    "signatures": ["Aw=="],
                })
                .to_string(),
            ),
            _ => (404, String::new()),
        })
        .await;
        let chain = attested_chain("http://127.0.0.1:1", vec![attestor.clone()], 1);
        let aggregator = AggregatorClient::new(vec![attestor], 1).unwrap();

        let payload = chain
            .build_update_client_payload_attested(&aggregator)
            .await
            .unwrap();
        assert_eq!(payload.headers.len(), 1);
        assert!(payload.target_execution_height.is_none());
        let proof: Value = serde_json::from_slice(&payload.headers[0]).unwrap();
        assert_eq!(
            proof,
            json!({
                "attested_data": [1, 2],
                "signatures": [[3]],
                "height": 55,
                "timestamp": 9,
            })
        );
    }

    #[tokio::test]
    async fn update_payload_fails_without_height_quorum() {
        let endpoints = vec!["http://127.0.0.1:1".to_string()];
        let chain = attested_chain("http://127.0.0.1:1", endpoints.clone(), 1);
        let aggregator = AggregatorClient::new(endpoints, 1).unwrap();
        let err = chain
            .build_update_client_payload_attested(&aggregator)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("latest height"), "{err:#}");
    }
}
