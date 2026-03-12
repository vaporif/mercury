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
use tracing::instrument;

use mercury_chain_traits::payload_builders::{
    CanBuildCreateClientPayload, CanBuildUpdateClientPayload,
};
use mercury_core::error::{Error, Result};
use tendermint::account;
use tendermint::block::Height as TmHeight;
use tendermint::validator::{Info as ValidatorInfo, Set as ValidatorSet};
use tendermint_rpc::{Client, Paging};

use crate::chain::CosmosChain;
use crate::keys::CosmosSigner;

const DEFAULT_TRUSTING_PERIOD: Duration = Duration::from_secs(14 * 24 * 3600);
const DEFAULT_UNBONDING_PERIOD: Duration = Duration::from_secs(21 * 24 * 3600);
const DEFAULT_MAX_CLOCK_DRIFT: Duration = Duration::from_secs(40);
const HEADER_FETCH_CONCURRENCY: usize = 8;

/// Payload for creating a Tendermint light client on a counterparty chain.
#[derive(Clone, Debug)]
pub struct CosmosCreateClientPayload {
    pub client_state: Any,
    pub consensus_state: Any,
}

/// Payload containing headers to update a Tendermint light client.
#[derive(Clone, Debug)]
pub struct CosmosUpdateClientPayload {
    pub headers: Vec<Any>,
}

#[async_trait]
impl<S: CosmosSigner> CanBuildCreateClientPayload<Self> for CosmosChain<S> {
    type CreateClientPayload = CosmosCreateClientPayload;

    #[instrument(skip_all, name = "build_create_client_payload")]
    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload> {
        let latest_block = self
            .rpc_client
            .latest_block()
            .await
            .map_err(Error::report)?;

        let latest_height = latest_block.block.header.height;

        let ibc_height = Height::new(self.chain_id.revision_number(), latest_height.value())
            .map_err(|e| Error::report(eyre::eyre!("{e}")))?;

        let trusting_period = self
            .config
            .trusting_period
            .unwrap_or(DEFAULT_TRUSTING_PERIOD);
        let unbonding_period = self
            .config
            .unbonding_period
            .unwrap_or(DEFAULT_UNBONDING_PERIOD);
        let max_clock_drift = self
            .config
            .max_clock_drift
            .unwrap_or(DEFAULT_MAX_CLOCK_DRIFT);

        let client_state = TendermintClientState::new(
            self.chain_id.clone(),
            TrustThreshold::ONE_THIRD,
            trusting_period,
            unbonding_period,
            max_clock_drift,
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
impl<S: CosmosSigner> CanBuildUpdateClientPayload<Self> for CosmosChain<S> {
    type UpdateClientPayload = CosmosUpdateClientPayload;

    #[instrument(skip_all, name = "build_update_client_payload", fields(trusted = %trusted_height, target = %target_height))]
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

        let (trusted_validators_response, trusted_commit_response) = tokio::try_join!(
            async {
                self.rpc_client
                    .validators(*trusted_height, Paging::All)
                    .await
                    .map_err(Error::report)
            },
            async {
                self.rpc_client
                    .commit(*trusted_height)
                    .await
                    .map_err(Error::report)
            },
        )?;

        let trusted_proposer = find_proposer(
            &trusted_validators_response.validators,
            &trusted_commit_response
                .signed_header
                .header
                .proposer_address,
        );
        let trusted_next_validator_set =
            ValidatorSet::new(trusted_validators_response.validators, trusted_proposer);

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

                    let proposer = find_proposer(
                        &validators_response.validators,
                        &commit_response.signed_header.header.proposer_address,
                    );
                    let validator_set = ValidatorSet::new(validators_response.validators, proposer);

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

fn find_proposer(
    validators: &[ValidatorInfo],
    proposer_address: &account::Id,
) -> Option<ValidatorInfo> {
    validators
        .iter()
        .find(|v| &v.address == proposer_address)
        .cloned()
}
