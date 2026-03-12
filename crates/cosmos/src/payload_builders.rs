use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, StreamExt, TryStreamExt};
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
use tendermint::block::Height as TmHeight;
use tendermint::validator::Set as ValidatorSet;
use tendermint_rpc::{Client, Paging};

use crate::chain::CosmosChain;

const TRUSTING_PERIOD: Duration = Duration::from_secs(14 * 24 * 3600);
const UNBONDING_PERIOD: Duration = Duration::from_secs(21 * 24 * 3600);
const MAX_CLOCK_DRIFT: Duration = Duration::from_secs(40);
const HEADER_FETCH_CONCURRENCY: usize = 8;

#[derive(Clone, Debug)]
pub struct CosmosCreateClientPayload {
    pub client_state: Any,
    pub consensus_state: Any,
}

#[derive(Clone, Debug)]
pub struct CosmosUpdateClientPayload {
    pub headers: Vec<Any>,
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
            TRUSTING_PERIOD,
            UNBONDING_PERIOD,
            MAX_CLOCK_DRIFT,
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

        Ok(CosmosCreateClientPayload {
            client_state: client_state.into(),
            consensus_state: consensus_state.into(),
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

        let trusted_validators_response = self
            .rpc_client
            .validators(*trusted_height, Paging::All)
            .await
            .map_err(Error::report)?;

        let trusted_next_validator_set =
            ValidatorSet::new(trusted_validators_response.validators, None);

        let ibc_trusted_height = Height::new(self.chain_id.revision_number(), trusted_height_value)
            .map_err(|e| Error::report(eyre::eyre!("{e}")))?;

        let heights: Vec<u64> = ((trusted_height_value + 1)..=target_height_value).collect();

        let headers: Vec<Any> = stream::iter(heights)
            .map(|h| {
                let rpc = &self.rpc_client;
                let trusted_vs = &trusted_next_validator_set;
                async move {
                    let height = TmHeight::try_from(h).map_err(Error::report)?;

                    let (commit_response, validators_response) = tokio::try_join!(
                        async { rpc.commit(height).await.map_err(Error::report) },
                        async {
                            rpc.validators(height, Paging::All)
                                .await
                                .map_err(Error::report)
                        },
                    )?;

                    let validator_set = ValidatorSet::new(validators_response.validators, None);

                    let header = TmIbcHeader {
                        signed_header: commit_response.signed_header,
                        validator_set,
                        trusted_height: ibc_trusted_height,
                        trusted_next_validator_set: trusted_vs.clone(),
                    };

                    Ok(header.into())
                }
            })
            .buffered(HEADER_FETCH_CONCURRENCY)
            .try_collect()
            .await?;

        Ok(CosmosUpdateClientPayload { headers })
    }
}
