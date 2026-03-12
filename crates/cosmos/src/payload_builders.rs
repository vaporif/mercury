use std::time::Duration;

use async_trait::async_trait;
use ibc::core::client::types::Height;
use ibc::core::commitment_types::specs::ProofSpecs;
use ibc_client_tendermint::types::{
    AllowUpdate, ClientState as TendermintClientState, ConsensusState as TendermintConsensusState,
    Header as TmIbcHeader, TrustThreshold,
};
use ibc_proto::google::protobuf::Any;
use mercury_chain_traits::payload_builders::{
    CanBuildCreateClientPayload, CanBuildUpdateClientPayload,
};
use mercury_core::error::{Error, Result};
use prost::Message;
use tendermint::block::Height as TmHeight;
use tendermint::validator::Set as ValidatorSet;
use tendermint_rpc::{Client, Paging};

use crate::chain::CosmosChain;

#[derive(Clone, Debug)]
pub struct CosmosCreateClientPayload {
    pub client_state_bytes: Vec<u8>,
    pub consensus_state_bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct CosmosUpdateClientPayload {
    pub headers: Vec<Vec<u8>>,
}

#[async_trait]
impl CanBuildCreateClientPayload<Self> for CosmosChain {
    type CreateClientPayload = CosmosCreateClientPayload;

    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload> {
        let latest_block = self
            .rpc_client
            .latest_block()
            .await
            .map_err(Error::report)?;

        let latest_height = latest_block.block.header.height;

        let ibc_height = Height::new(self.chain_id.revision_number(), latest_height.value())
            .map_err(|e| Error::report(eyre::eyre!("{e}")))?;

        let client_state = TendermintClientState::new(
            self.chain_id.clone(),
            TrustThreshold::ONE_THIRD,
            Duration::from_secs(14 * 24 * 3600), // 14 days trusting period
            Duration::from_secs(21 * 24 * 3600), // 21 days unbonding period
            Duration::from_secs(40),             // 40s max clock drift
            ibc_height,
            ProofSpecs::cosmos(),
            vec!["upgrade".to_string(), "upgradedIBCState".to_string()],
            AllowUpdate {
                after_expiry: true,
                after_misbehaviour: true,
            },
        )
        .map_err(|e| Error::report(eyre::eyre!("{e}")))?;

        let consensus_state = TendermintConsensusState::from(latest_block.block.header);

        let client_state_any: Any = client_state.into();
        let consensus_state_any: Any = consensus_state.into();

        let client_state_bytes = client_state_any.encode_to_vec();
        let consensus_state_bytes = consensus_state_any.encode_to_vec();

        Ok(CosmosCreateClientPayload {
            client_state_bytes,
            consensus_state_bytes,
        })
    }
}

#[async_trait]
impl CanBuildUpdateClientPayload<Self> for CosmosChain {
    type UpdateClientPayload = CosmosUpdateClientPayload;

    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
    ) -> Result<Self::UpdateClientPayload> {
        let trusted_height_value = trusted_height.value();
        let target_height_value = target_height.value();

        if target_height_value <= trusted_height_value {
            return Err(Error::report(eyre::eyre!(
                "target height ({target_height_value}) must be greater than trusted height ({trusted_height_value})"
            )));
        }

        // Fetch the trusted validators (next_validator_set at trusted height)
        let trusted_validators_response = self
            .rpc_client
            .validators(*trusted_height, Paging::All)
            .await
            .map_err(Error::report)?;

        let trusted_next_validator_set =
            ValidatorSet::new(trusted_validators_response.validators, None);

        let ibc_trusted_height = Height::new(self.chain_id.revision_number(), trusted_height_value)
            .map_err(|e| Error::report(eyre::eyre!("{e}")))?;

        let mut headers = Vec::new();

        // Build a header for each height from trusted_height+1 to target_height
        for h in (trusted_height_value + 1)..=target_height_value {
            let height = TmHeight::try_from(h).map_err(Error::report)?;

            let commit_response = self
                .rpc_client
                .commit(height)
                .await
                .map_err(Error::report)?;

            let validators_response = self
                .rpc_client
                .validators(height, Paging::All)
                .await
                .map_err(Error::report)?;

            let validator_set = ValidatorSet::new(validators_response.validators, None);

            let header = TmIbcHeader {
                signed_header: commit_response.signed_header,
                validator_set,
                trusted_height: ibc_trusted_height,
                trusted_next_validator_set: trusted_next_validator_set.clone(),
            };

            let header_any: Any = header.into();
            headers.push(header_any.encode_to_vec());
        }

        Ok(CosmosUpdateClientPayload { headers })
    }
}
