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

    pub(crate) fn build_update_client_payload_mock(
        trusted_slot: u64,
    ) -> mercury_core::error::Result<UpdateClientPayload> {
        let overflow = || eyre::eyre!("mock update: slot {trusted_slot} too large to advance");
        let target_slot = trusted_slot.checked_add(1).ok_or_else(overflow)?;
        let signature_slot = target_slot.checked_add(1).ok_or_else(overflow)?;
        let timestamp = target_slot.checked_mul(12).ok_or_else(overflow)?;

        let finalized_header = LightClientHeader {
            beacon: BeaconBlockHeader {
                slot: target_slot,
                ..Default::default()
            },
            execution: ExecutionPayloadHeader {
                block_number: target_slot,
                timestamp,
                ..Default::default()
            },
            execution_branch: [B256::ZERO; 4],
        };

        let attested_header = LightClientHeader {
            beacon: BeaconBlockHeader {
                slot: signature_slot,
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
            signature_slot,
        };

        let header = EthHeader {
            active_sync_committee: ActiveSyncCommittee::Current(SyncCommittee {
                pubkeys: vec![],
                aggregate_pubkey: alloy::primitives::FixedBytes::default(),
            }),
            consensus_update,
            trusted_slot,
        };

        let header_bytes = serde_json::to_vec(&header).wrap_err("serializing mock header")?;

        Ok(UpdateClientPayload {
            headers: vec![header_bytes],
            target_execution_height: None,
            target_slot: None,
            required_dst_timestamp_secs: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::test_support::{CHAIN_ID, ROUTER, offline_chain};

    #[test]
    fn create_payload_encodes_mock_states() {
        let payload = offline_chain().build_create_client_payload_mock().unwrap();

        let cs: EthClientState = serde_json::from_slice(&payload.client_state).unwrap();
        assert_eq!(cs.chain_id, CHAIN_ID);
        assert_eq!(cs.ibc_contract_address, ROUTER);
        assert_eq!(cs.latest_slot, 1);
        assert_eq!(cs.min_sync_committee_participants, 1);
        assert_eq!(cs.sync_committee_size, 512);

        let cons: EthConsensusState = serde_json::from_slice(&payload.consensus_state).unwrap();
        assert_eq!(cons.slot, cs.latest_slot);
        assert!(cons.next_sync_committee.is_none());
        assert!(payload.counterparty_client_id.is_none());
        assert!(payload.counterparty_merkle_prefix.is_none());
    }

    #[test]
    fn update_payload_advances_one_slot_past_trusted() {
        let payload = EthereumChain::build_update_client_payload_mock(41).unwrap();
        assert_eq!(payload.headers.len(), 1);
        assert!(payload.target_execution_height.is_none());
        assert!(payload.target_slot.is_none());
        assert!(payload.required_dst_timestamp_secs.is_none());

        let header: EthHeader = serde_json::from_slice(&payload.headers[0]).unwrap();
        assert_eq!(header.trusted_slot, 41);
        let update = &header.consensus_update;
        assert_eq!(update.finalized_header.beacon.slot, 42);
        assert_eq!(update.finalized_header.execution.block_number, 42);
        assert_eq!(update.finalized_header.execution.timestamp, 42 * 12);
        assert_eq!(update.attested_header.beacon.slot, 43);
        assert_eq!(update.signature_slot, 43);
        assert!(matches!(
            header.active_sync_committee,
            ActiveSyncCommittee::Current(_)
        ));
    }

    #[test]
    fn update_payload_rejects_slot_overflow() {
        for slot in [u64::MAX, u64::MAX - 1, u64::MAX / 12] {
            assert!(EthereumChain::build_update_client_payload_mock(slot).is_err());
        }
    }
}
