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
            attestor_endpoints,
            quorum_threshold,
        } = &self.config.client_payload_mode
        else {
            unreachable!("attested path called in non-attested mode")
        };

        let client_state = serde_json::json!({
            "height": height,
            "timestamp": timestamp,
            "attestor_addresses": attestor_endpoints,
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
        })
    }
}
