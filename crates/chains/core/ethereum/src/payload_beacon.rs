use eyre::Context;
use tracing::info;

use ethereum_apis::beacon_api::client::BeaconApiClient;
use ethereum_light_client::client_state::ClientState as EthClientState;
use ethereum_light_client::consensus_state::ConsensusState as EthConsensusState;
use ethereum_light_client::header::{ActiveSyncCommittee, Header as EthHeader};

use crate::builders::{CreateClientPayload, UpdateClientPayload};
use crate::chain::EthereumChain;
use crate::types::EvmClientState;

struct PeriodCrossingResult {
    headers: Vec<Vec<u8>>,
    trusted_slot: u64,
}

impl EthereumChain {
    pub(crate) async fn build_create_client_payload_beacon(
        &self,
        beacon_api: &BeaconApiClient,
    ) -> mercury_core::error::Result<CreateClientPayload> {
        let genesis = beacon_api.genesis().await.wrap_err("beacon API genesis")?;
        let spec = beacon_api.spec().await.wrap_err("beacon API spec")?;

        let finality = beacon_api
            .finality_update()
            .await
            .wrap_err("beacon API finality_update")?;
        let finalized_slot = finality.data.finalized_header.beacon.slot;
        let finalized_block_number = finality.data.finalized_header.execution.block_number;

        let block_root = beacon_api
            .beacon_block_root(&finalized_slot.to_string())
            .await
            .wrap_err("beacon API block_root")?;
        let bootstrap = beacon_api
            .light_client_bootstrap(&block_root)
            .await
            .wrap_err("beacon API bootstrap")?;

        let client_state = EthClientState {
            chain_id: self.config.chain_id,
            genesis_validators_root: genesis.data.genesis_validators_root,
            min_sync_committee_participants: 1,
            sync_committee_size: spec.data.sync_committee_size,
            genesis_time: genesis.data.genesis_time,
            genesis_slot: spec.data.genesis_slot,
            fork_parameters: spec.data.to_fork_parameters(),
            seconds_per_slot: spec.data.seconds_per_slot,
            slots_per_epoch: spec.data.slots_per_epoch,
            epochs_per_sync_committee_period: spec.data.epochs_per_sync_committee_period,
            latest_slot: finalized_slot,
            latest_execution_block_number: finalized_block_number,
            is_frozen: false,
            ibc_contract_address: self.router_address,
            ibc_commitment_slot: crate::ics24::IBC_STORE_COMMITMENTS_SLOT,
        };

        let current_period = client_state.compute_sync_committee_period_at_slot(finalized_slot);
        let next_sync_committee = beacon_api
            .light_client_updates(current_period, 1)
            .await
            .ok()
            .and_then(|mut updates| updates.pop())
            .and_then(|resp| resp.data.next_sync_committee)
            .map(|c| c.to_summarized_sync_committee());

        if next_sync_committee.is_some() {
            info!("including next_sync_committee in initial consensus state");
        } else {
            info!("next_sync_committee not yet available from beacon API");
        }

        let consensus_state = EthConsensusState {
            slot: finalized_slot,
            state_root: finality.data.finalized_header.execution.state_root,
            timestamp: finality.data.finalized_header.execution.timestamp,
            current_sync_committee: bootstrap
                .data
                .current_sync_committee
                .to_summarized_sync_committee(),
            next_sync_committee,
        };

        let client_state_bytes =
            serde_json::to_vec(&client_state).wrap_err("serializing client state")?;
        let consensus_state_bytes =
            serde_json::to_vec(&consensus_state).wrap_err("serializing consensus state")?;

        Ok(CreateClientPayload {
            client_state: client_state_bytes,
            consensus_state: consensus_state_bytes,
            counterparty_client_id: None,
            counterparty_merkle_prefix: None,
        })
    }

    pub(crate) async fn build_update_client_payload_beacon(
        &self,
        beacon_api: &BeaconApiClient,
        counterparty_client_state: &EvmClientState,
    ) -> mercury_core::error::Result<UpdateClientPayload> {
        let eth_client_state: EthClientState = serde_json::from_slice(&counterparty_client_state.0)
            .wrap_err("decoding counterparty ethereum client state")?;
        let trusted_slot = eth_client_state.latest_slot;

        let finality = beacon_api
            .finality_update()
            .await
            .wrap_err("beacon API finality_update")?;
        let target_slot = finality.data.finalized_header.beacon.slot;

        if target_slot <= trusted_slot {
            return Ok(UpdateClientPayload { headers: vec![] });
        }

        let trusted_period = eth_client_state.compute_sync_committee_period_at_slot(trusted_slot);
        let target_period = eth_client_state.compute_sync_committee_period_at_slot(target_slot);

        let mut headers = Vec::new();
        let mut current_trusted_slot = trusted_slot;

        if target_period > trusted_period {
            let result = self
                .build_period_crossing_headers(
                    beacon_api,
                    &eth_client_state,
                    trusted_period,
                    target_period,
                    current_trusted_slot,
                )
                .await?;
            headers = result.headers;
            current_trusted_slot = result.trusted_slot;
        }

        if finality.data.finalized_header.beacon.slot > current_trusted_slot {
            self.build_finality_header(
                beacon_api,
                finality.data,
                current_trusted_slot,
                &mut headers,
            )
            .await?;
        }

        Ok(UpdateClientPayload { headers })
    }

    async fn build_period_crossing_headers(
        &self,
        beacon_api: &BeaconApiClient,
        eth_client_state: &EthClientState,
        trusted_period: u64,
        target_period: u64,
        initial_trusted_slot: u64,
    ) -> mercury_core::error::Result<PeriodCrossingResult> {
        let count = target_period - trusted_period + 1;
        let updates = beacon_api
            .light_client_updates(trusted_period, count)
            .await
            .wrap_err("beacon API light_client_updates")?;

        let mut headers = Vec::new();
        let mut current_trusted_slot = initial_trusted_slot;
        let mut latest_period = trusted_period;

        for update_response in updates {
            let update = update_response.data;
            let update_finalized_slot = update.finalized_header.beacon.slot;

            if update_finalized_slot <= current_trusted_slot {
                continue;
            }

            let update_period =
                eth_client_state.compute_sync_committee_period_at_slot(update_finalized_slot);

            if update_period == latest_period {
                continue;
            }

            let block_root = beacon_api
                .beacon_block_root(&update_finalized_slot.to_string())
                .await
                .wrap_err("beacon API block_root for period crossing")?;
            let bootstrap = beacon_api
                .light_client_bootstrap(&block_root)
                .await
                .wrap_err("beacon API bootstrap for period crossing")?;

            let header = EthHeader {
                active_sync_committee: ActiveSyncCommittee::Next(
                    bootstrap.data.current_sync_committee,
                ),
                consensus_update: update,
                trusted_slot: current_trusted_slot,
            };

            info!(
                from_period = latest_period,
                to_period = update_period,
                slot = update_finalized_slot,
                "adding period crossing header"
            );

            let header_bytes = serde_json::to_vec(&header).wrap_err("serializing beacon header")?;
            headers.push(header_bytes);

            current_trusted_slot = update_finalized_slot;
            latest_period = update_period;
        }

        Ok(PeriodCrossingResult {
            headers,
            trusted_slot: current_trusted_slot,
        })
    }

    async fn build_finality_header(
        &self,
        beacon_api: &BeaconApiClient,
        finality: ethereum_types::consensus::light_client_header::LightClientFinalityUpdate,
        current_trusted_slot: u64,
        headers: &mut Vec<Vec<u8>>,
    ) -> mercury_core::error::Result<()> {
        let attested_slot = finality.attested_header.beacon.slot;
        let block_root = beacon_api
            .beacon_block_root(&attested_slot.to_string())
            .await
            .wrap_err("beacon API block_root for finality update")?;
        let bootstrap = beacon_api
            .light_client_bootstrap(&block_root)
            .await
            .wrap_err("beacon API bootstrap for finality update")?;

        let header = EthHeader {
            active_sync_committee: ActiveSyncCommittee::Current(
                bootstrap.data.current_sync_committee,
            ),
            consensus_update: finality.into(),
            trusted_slot: current_trusted_slot,
        };

        let header_bytes = serde_json::to_vec(&header).wrap_err("serializing finality header")?;
        headers.push(header_bytes);

        Ok(())
    }
}
