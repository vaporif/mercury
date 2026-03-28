use alloy::primitives::B256;
use eyre::Context;

use ethereum_light_client::client_state::ClientState as EthClientState;
use ethereum_light_client::consensus_state::ConsensusState as EthConsensusState;
use ethereum_light_client::header::{ActiveSyncCommittee, Header as EthHeader};
use ethereum_types::consensus::light_client_header::{
    BeaconBlockHeader, ExecutionPayloadHeader, LightClientHeader, LightClientUpdate,
};
use ethereum_types::consensus::sync_committee::{
    SummarizedSyncCommittee, SyncAggregate, SyncCommittee,
};

use crate::builders::{CreateClientPayload, UpdateClientPayload};
use crate::chain::EthereumChain;

impl EthereumChain {
    pub(crate) fn build_create_client_payload_mock(
        &self,
    ) -> mercury_core::error::Result<CreateClientPayload> {
        let initial_slot = 1u64;

        let state = EthClientState {
            chain_id: self.chain_id.0,
            ibc_contract_address: self.router_address,
            latest_slot: initial_slot,
            min_sync_committee_participants: 1,
            sync_committee_size: 512,
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            seconds_per_slot: 12,
            ..EthClientState::default()
        };

        let consensus_state = EthConsensusState {
            slot: initial_slot,
            state_root: B256::ZERO,
            timestamp: 1,
            current_sync_committee: SummarizedSyncCommittee::default(),
            next_sync_committee: None,
        };

        let client_state = serde_json::to_vec(&state).wrap_err("serializing mock client state")?;
        let consensus_state =
            serde_json::to_vec(&consensus_state).wrap_err("serializing mock consensus state")?;

        Ok(CreateClientPayload {
            client_state,
            consensus_state,
            counterparty_client_id: None,
            counterparty_merkle_prefix: None,
        })
    }

    pub(crate) fn build_update_client_payload_mock(trusted_slot: u64) -> UpdateClientPayload {
        let target_slot = trusted_slot + 1;

        let finalized_header = LightClientHeader {
            beacon: BeaconBlockHeader {
                slot: target_slot,
                ..Default::default()
            },
            execution: ExecutionPayloadHeader {
                block_number: target_slot,
                timestamp: target_slot * 12,
                ..Default::default()
            },
            execution_branch: [B256::ZERO; 4],
        };

        let attested_header = LightClientHeader {
            beacon: BeaconBlockHeader {
                slot: target_slot + 1,
                ..Default::default()
            },
            ..Default::default()
        };

        let consensus_update = LightClientUpdate {
            attested_header,
            next_sync_committee: None,
            next_sync_committee_branch: None,
            finalized_header,
            finality_branch: [B256::ZERO; 7],
            sync_aggregate: SyncAggregate::default(),
            signature_slot: target_slot + 1,
        };

        let header = EthHeader {
            active_sync_committee: ActiveSyncCommittee::Current(SyncCommittee {
                pubkeys: vec![],
                aggregate_pubkey: alloy::primitives::FixedBytes::default(),
            }),
            consensus_update,
            trusted_slot,
        };

        let header_bytes =
            serde_json::to_vec(&header).expect("mock EthHeader serialization cannot fail");

        UpdateClientPayload {
            headers: vec![header_bytes],
            target_execution_height: None,
        }
    }
}
