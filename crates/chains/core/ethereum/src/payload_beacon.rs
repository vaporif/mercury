use eyre::Context;
use tracing::info;

use ethereum_apis::beacon_api::client::BeaconApiClient;
use ethereum_light_client::client_state::ClientState as EthClientState;
use ethereum_light_client::consensus_state::ConsensusState as EthConsensusState;
use ethereum_light_client::header::{ActiveSyncCommittee, Header as EthHeader};

use crate::builders::{CreateClientPayload, UpdateClientPayload};
use crate::chain::EthereumChain;
use crate::types::{EvmClientState, EvmHeight};

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

        let target_execution_height = finality.data.finalized_header.execution.block_number;

        if target_slot <= trusted_slot {
            eyre::bail!(
                "beacon finality has not advanced past trusted slot {trusted_slot} \
                 (current finalized slot: {target_slot}), deferring"
            );
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

        let finality_signature_slot = finality.data.signature_slot;

        if finality.data.finalized_header.beacon.slot > current_trusted_slot {
            self.build_finality_header(
                beacon_api,
                finality.data,
                current_trusted_slot,
                &mut headers,
            )
            .await?;
        }

        let signature_slot_timestamp = eth_client_state.genesis_time
            + finality_signature_slot.saturating_sub(eth_client_state.genesis_slot)
                * eth_client_state.seconds_per_slot;

        Ok(UpdateClientPayload {
            headers,
            target_execution_height: Some(EvmHeight(target_execution_height)),
            target_slot: Some(target_slot),
            required_dst_timestamp_secs: Some(signature_slot_timestamp),
        })
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use alloy::primitives::{B256, FixedBytes};
    use ethereum_types::consensus::bootstrap::LightClientBootstrap;
    use ethereum_types::consensus::genesis::Genesis;
    use ethereum_types::consensus::light_client_header::{
        LightClientFinalityUpdate, LightClientUpdate,
    };
    use ethereum_types::consensus::spec::Spec;
    use ethereum_types::consensus::sync_committee::SyncCommittee;
    use serde_json::json;

    use super::*;
    use crate::chain::test_support::{CHAIN_ID, ROUTER, offline_chain, spawn_http};

    const SLOTS_PER_PERIOD: u64 = 32 * 256;
    const GENESIS_TIME: u64 = 1_000;

    fn committee(slot: u64) -> SyncCommittee {
        SyncCommittee {
            pubkeys: vec![],
            aggregate_pubkey: FixedBytes::left_padding_from(&slot.to_be_bytes()),
        }
    }

    fn spec() -> Spec {
        Spec {
            seconds_per_slot: 12,
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            ..Spec::default()
        }
    }

    fn update_at(finalized_slot: u64) -> LightClientUpdate {
        let mut u = LightClientUpdate::default();
        u.finalized_header.beacon.slot = finalized_slot;
        u.attested_header.beacon.slot = finalized_slot + 64;
        u.signature_slot = finalized_slot + 65;
        u
    }

    #[derive(Clone)]
    struct Fixture {
        finalized_slot: u64,
        updates: Option<Vec<LightClientUpdate>>,
        fail_genesis: bool,
    }

    impl Fixture {
        const fn new(finalized_slot: u64) -> Self {
            Self {
                finalized_slot,
                updates: Some(vec![]),
                fail_genesis: false,
            }
        }

        fn finality(&self) -> LightClientFinalityUpdate {
            let mut f = LightClientFinalityUpdate::default();
            f.finalized_header.beacon.slot = self.finalized_slot;
            f.finalized_header.execution.block_number = self.finalized_slot * 10;
            f.finalized_header.execution.state_root = B256::repeat_byte(0x5e);
            f.finalized_header.execution.timestamp = 4_242;
            f.attested_header.beacon.slot = self.finalized_slot + 64;
            f.signature_slot = self.finalized_slot + 65;
            f
        }
    }

    /// Mock beacon node. Block roots encode the slot, and each bootstrap
    /// returns a committee keyed by that slot so tests can tell which
    /// bootstrap fed which header. Returns the URL and recorded update queries.
    async fn beacon_server(fx: Fixture) -> (String, Arc<Mutex<Vec<String>>>) {
        let update_queries = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&update_queries);
        let url = spawn_http(move |req| {
            let path = req.path.as_str();
            let ok = |v: serde_json::Value| (200, v.to_string());
            if path == "/eth/v1/beacon/genesis" {
                if fx.fail_genesis {
                    return (503, "unavailable".to_string());
                }
                let genesis = Genesis {
                    genesis_time: GENESIS_TIME,
                    genesis_validators_root: B256::repeat_byte(0x0a),
                    ..Genesis::default()
                };
                ok(json!({ "data": genesis }))
            } else if path == "/eth/v1/config/spec" {
                ok(json!({ "data": spec() }))
            } else if path == "/eth/v1/beacon/light_client/finality_update" {
                ok(json!({ "version": "electra", "data": fx.finality() }))
            } else if let Some(rest) = path.strip_prefix("/eth/v1/beacon/blocks/") {
                let slot: u64 = rest.trim_end_matches("/root").parse().unwrap();
                ok(json!({ "data": { "root": format!("0x{slot:064x}") } }))
            } else if let Some(root) = path.strip_prefix("/eth/v1/beacon/light_client/bootstrap/") {
                let slot = u64::from_str_radix(root.trim_start_matches("0x"), 16).unwrap();
                let bootstrap = LightClientBootstrap {
                    current_sync_committee: committee(slot),
                    ..LightClientBootstrap::default()
                };
                ok(json!({ "data": bootstrap }))
            } else if let Some(query) = path.strip_prefix("/eth/v1/beacon/light_client/updates?") {
                recorded.lock().unwrap().push(query.to_string());
                fx.updates.as_ref().map_or_else(
                    || (400, "no updates".to_string()),
                    |updates| {
                        let wrapped: Vec<_> =
                            updates.iter().map(|u| json!({ "data": u })).collect();
                        ok(json!(wrapped))
                    },
                )
            } else {
                (404, format!("{{\"code\":404,\"message\":\"{path}\"}}"))
            }
        })
        .await;
        (url, update_queries)
    }

    fn client(url: String) -> BeaconApiClient {
        BeaconApiClient::new(url)
    }

    fn trusted_state(latest_slot: u64) -> EvmClientState {
        let cs = EthClientState {
            latest_slot,
            genesis_time: GENESIS_TIME,
            genesis_slot: 0,
            seconds_per_slot: 12,
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            ..EthClientState::default()
        };
        EvmClientState(serde_json::to_vec(&cs).unwrap())
    }

    fn decode_headers(payload: &UpdateClientPayload) -> Vec<EthHeader> {
        payload
            .headers
            .iter()
            .map(|h| serde_json::from_slice(h).unwrap())
            .collect()
    }

    fn assert_committee(active: &ActiveSyncCommittee, next: bool, slot: u64) {
        match (active, next) {
            (ActiveSyncCommittee::Next(c), true) | (ActiveSyncCommittee::Current(c), false) => {
                assert_eq!(c, &committee(slot));
            }
            _ => panic!("unexpected committee kind {active:?}, wanted next={next}"),
        }
    }

    #[tokio::test]
    async fn create_payload_builds_states_from_beacon() {
        let finalized = 300;
        let mut fx = Fixture::new(finalized);
        let mut next = update_at(200);
        next.next_sync_committee = Some(committee(999));
        fx.updates = Some(vec![next]);
        let (url, queries) = beacon_server(fx).await;

        let payload = offline_chain()
            .build_create_client_payload_beacon(&client(url))
            .await
            .unwrap();

        let cs: EthClientState = serde_json::from_slice(&payload.client_state).unwrap();
        assert_eq!(cs.chain_id, CHAIN_ID);
        assert_eq!(cs.genesis_validators_root, B256::repeat_byte(0x0a));
        assert_eq!(cs.genesis_time, GENESIS_TIME);
        assert_eq!(cs.sync_committee_size, 512);
        assert_eq!(cs.slots_per_epoch, 32);
        assert_eq!(cs.epochs_per_sync_committee_period, 256);
        assert_eq!(cs.seconds_per_slot, 12);
        assert_eq!(cs.latest_slot, finalized);
        assert_eq!(cs.latest_execution_block_number, finalized * 10);
        assert_eq!(cs.ibc_contract_address, ROUTER);
        assert_eq!(
            cs.ibc_commitment_slot,
            crate::ics24::IBC_STORE_COMMITMENTS_SLOT
        );
        assert!(!cs.is_frozen);

        let cons: EthConsensusState = serde_json::from_slice(&payload.consensus_state).unwrap();
        assert_eq!(cons.slot, finalized);
        assert_eq!(cons.state_root, B256::repeat_byte(0x5e));
        assert_eq!(cons.timestamp, 4_242);
        assert_eq!(
            cons.current_sync_committee,
            committee(finalized).to_summarized_sync_committee()
        );
        assert_eq!(
            cons.next_sync_committee,
            Some(committee(999).to_summarized_sync_committee())
        );
        assert!(payload.counterparty_client_id.is_none());
        assert_eq!(*queries.lock().unwrap(), vec!["start_period=0&count=1"]);
    }

    #[tokio::test]
    async fn create_payload_tolerates_missing_next_committee() {
        let mut fx = Fixture::new(SLOTS_PER_PERIOD + 5);
        fx.updates = None;
        let (url, queries) = beacon_server(fx).await;
        let payload = offline_chain()
            .build_create_client_payload_beacon(&client(url))
            .await
            .unwrap();
        let cons: EthConsensusState = serde_json::from_slice(&payload.consensus_state).unwrap();
        assert!(cons.next_sync_committee.is_none());
        assert_eq!(*queries.lock().unwrap(), vec!["start_period=1&count=1"]);
    }

    #[tokio::test]
    async fn create_payload_fails_when_genesis_unavailable() {
        let mut fx = Fixture::new(10);
        fx.fail_genesis = true;
        let (url, _) = beacon_server(fx).await;
        let err = offline_chain()
            .build_create_client_payload_beacon(&client(url))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("genesis"), "{err}");
    }

    #[tokio::test]
    async fn update_payload_rejects_undecodable_client_state() {
        let (url, _) = beacon_server(Fixture::new(10)).await;
        let err = offline_chain()
            .build_update_client_payload_beacon(&client(url), &EvmClientState(vec![1, 2]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("client state"), "{err}");
    }

    #[tokio::test]
    async fn update_payload_defers_until_finality_advances() {
        for finalized in [50, 100] {
            let (url, _) = beacon_server(Fixture::new(finalized)).await;
            let err = offline_chain()
                .build_update_client_payload_beacon(&client(url), &trusted_state(100))
                .await
                .unwrap_err();
            assert!(err.to_string().contains("deferring"), "{err}");
        }
    }

    #[tokio::test]
    async fn update_within_period_emits_single_finality_header() {
        let finalized = 200;
        let (url, queries) = beacon_server(Fixture::new(finalized)).await;
        let payload = offline_chain()
            .build_update_client_payload_beacon(&client(url), &trusted_state(100))
            .await
            .unwrap();

        let headers = decode_headers(&payload);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].trusted_slot, 100);
        assert_committee(&headers[0].active_sync_committee, false, finalized + 64);
        assert_eq!(
            headers[0].consensus_update.finalized_header.beacon.slot,
            finalized
        );
        assert!(headers[0].consensus_update.next_sync_committee.is_none());

        assert_eq!(
            payload.target_execution_height,
            Some(EvmHeight(finalized * 10))
        );
        assert_eq!(payload.target_slot, Some(finalized));
        assert_eq!(
            payload.required_dst_timestamp_secs,
            Some(GENESIS_TIME + (finalized + 65) * 12)
        );
        assert!(queries.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn update_across_periods_chains_headers() {
        let trusted = 100;
        let p1_slot = SLOTS_PER_PERIOD + 8;
        let p2_slot = 2 * SLOTS_PER_PERIOD + 16;
        let finalized = 2 * SLOTS_PER_PERIOD + 200;
        let mut fx = Fixture::new(finalized);
        fx.updates = Some(vec![
            update_at(50),  // behind trusted slot: skipped
            update_at(150), // same period as trusted: skipped
            update_at(p1_slot),
            update_at(p2_slot),
        ]);
        let (url, queries) = beacon_server(fx).await;

        let payload = offline_chain()
            .build_update_client_payload_beacon(&client(url), &trusted_state(trusted))
            .await
            .unwrap();

        assert_eq!(*queries.lock().unwrap(), vec!["start_period=0&count=3"]);
        let headers = decode_headers(&payload);
        assert_eq!(headers.len(), 3);

        assert_eq!(headers[0].trusted_slot, trusted);
        assert_committee(&headers[0].active_sync_committee, true, p1_slot);
        assert_eq!(
            headers[0].consensus_update.finalized_header.beacon.slot,
            p1_slot
        );

        assert_eq!(headers[1].trusted_slot, p1_slot);
        assert_committee(&headers[1].active_sync_committee, true, p2_slot);

        assert_eq!(headers[2].trusted_slot, p2_slot);
        assert_committee(&headers[2].active_sync_committee, false, finalized + 64);
        assert_eq!(payload.target_slot, Some(finalized));
    }

    #[tokio::test]
    async fn update_skips_finality_header_when_crossing_reached_target() {
        let finalized = SLOTS_PER_PERIOD + 8;
        let mut fx = Fixture::new(finalized);
        fx.updates = Some(vec![update_at(finalized)]);
        let (url, _) = beacon_server(fx).await;

        let payload = offline_chain()
            .build_update_client_payload_beacon(&client(url), &trusted_state(10))
            .await
            .unwrap();
        let headers = decode_headers(&payload);
        assert_eq!(headers.len(), 1);
        assert_committee(&headers[0].active_sync_committee, true, finalized);
        assert_eq!(payload.target_slot, Some(finalized));
    }

    #[tokio::test]
    async fn update_fails_when_period_updates_unavailable() {
        let mut fx = Fixture::new(SLOTS_PER_PERIOD + 8);
        fx.updates = None;
        let (url, _) = beacon_server(fx).await;
        let err = offline_chain()
            .build_update_client_payload_beacon(&client(url), &trusted_state(10))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("light_client_updates"), "{err}");
    }
}
