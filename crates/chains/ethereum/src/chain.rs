use std::sync::Arc;
use std::time::Duration;

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use eyre::Context;
use tracing::info;

use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, UpdateClientOutput,
};
use mercury_chain_traits::queries::ChainStatusQuery;
use mercury_chain_traits::types::{ChainTypes, IbcTypes};

use crate::aggregator::AggregatorClient;
use crate::builders::{CreateClientPayload, UpdateClientPayload};
use crate::config::{ClientPayloadMode, EthereumChainConfig};
use crate::contracts::{ICS26Router, IICS02ClientMsgs};
use crate::types::{
    EvmAcknowledgement, EvmChainId, EvmChainStatus, EvmClientId, EvmClientState,
    EvmCommitmentProof, EvmConsensusState, EvmEvent, EvmHeight, EvmMessage, EvmPacket,
    EvmPacketCommitment, EvmPacketReceipt, EvmTimestamp, EvmTxResponse,
};

#[cfg(feature = "sp1")]
use mercury_core::MembershipProofs;

use ethereum_apis::beacon_api::client::BeaconApiClient;

#[derive(Clone)]
pub enum PayloadClient {
    Beacon(Arc<BeaconApiClient>),
    Attested(AggregatorClient),
    Mock,
}

impl std::fmt::Debug for PayloadClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Beacon(_) => f.debug_tuple("Beacon").finish(),
            Self::Attested(c) => f.debug_tuple("Attested").field(c).finish(),
            Self::Mock => f.debug_tuple("Mock").finish(),
        }
    }
}

#[derive(Clone)]
pub struct EthereumChain {
    pub config: EthereumChainConfig,
    pub chain_id: EvmChainId,
    pub router_address: Address,
    pub provider: Arc<DynProvider>,
    pub rpc_guard: mercury_core::rpc_guard::RpcGuard,
    pub payload_client: PayloadClient,
    #[cfg(feature = "sp1")]
    pub sp1: Option<Arc<crate::sp1::Sp1Instance<sp1_prover::components::CpuProverComponents>>>,
}

impl std::fmt::Debug for EthereumChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthereumChain")
            .field("chain_id", &self.chain_id)
            .field("router_address", &self.router_address)
            .field("rpc_guard", &self.rpc_guard)
            .field("payload_client", &self.payload_client)
            .finish_non_exhaustive()
    }
}

impl EthereumChain {
    pub async fn new(
        config: EthereumChainConfig,
        signer: alloy::signers::local::PrivateKeySigner,
    ) -> mercury_core::error::Result<Self> {
        let wallet = EthereumWallet::from(signer);
        let url: url::Url = config.rpc_addr.parse().wrap_err("parsing RPC URL")?;
        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .connect_http(url)
            .erased();

        let rpc_config = config.rpc_config();
        rpc_config.validate().wrap_err("invalid RPC config")?;
        let rpc_guard = mercury_core::rpc_guard::RpcGuard::new(&config.chain_id.to_string(), rpc_config);

        let on_chain_id: u64 = rpc_guard
            .guarded(|| async { provider.get_chain_id().await.wrap_err("querying chain ID") })
            .await?;

        if on_chain_id != config.chain_id {
            eyre::bail!(
                "chain_id mismatch: config says {}, node reports {}",
                config.chain_id,
                on_chain_id,
            );
        }

        let sync_status = rpc_guard
            .guarded(|| async { provider.syncing().await.wrap_err("querying sync status") })
            .await?;
        if !matches!(sync_status, alloy::rpc::types::SyncStatus::None) {
            eyre::bail!(
                "chain '{}': node is still syncing — wait for it to catch up before starting the relayer",
                config.chain_id,
            );
        }

        let router_address = config.router_address()?;

        let payload_client = match &config.client_payload_mode {
            ClientPayloadMode::Beacon { beacon_api_url } => {
                PayloadClient::Beacon(Arc::new(BeaconApiClient::new(beacon_api_url.clone())))
            }
            ClientPayloadMode::Attested {
                attestor_endpoints,
                quorum_threshold,
            } => PayloadClient::Attested(AggregatorClient::new(
                attestor_endpoints.clone(),
                *quorum_threshold,
            )),
            ClientPayloadMode::Mock => PayloadClient::Mock,
        };

        #[cfg(feature = "sp1")]
        let sp1 = if let Some(ref sp1_config) = config.sp1_prover {
            Some(Arc::new(crate::sp1::create_sp1_instance(sp1_config)?))
        } else {
            None
        };

        info!(chain_id = config.chain_id, %router_address, "ethereum chain initialized");

        Ok(Self {
            chain_id: EvmChainId(config.chain_id),
            config,
            router_address,
            provider: Arc::new(provider),
            rpc_guard,
            payload_client,
            #[cfg(feature = "sp1")]
            sp1,
        })
    }
}

impl ChainTypes for EthereumChain {
    type Height = EvmHeight;
    type Timestamp = EvmTimestamp;
    type ChainId = EvmChainId;
    type ClientId = EvmClientId;
    type Event = EvmEvent;
    type Message = EvmMessage;
    type MessageResponse = EvmTxResponse;
    type ChainStatus = EvmChainStatus;

    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height {
        &status.height
    }

    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp {
        &status.timestamp
    }

    fn chain_status_timestamp_secs(status: &Self::ChainStatus) -> u64 {
        status.timestamp.0
    }

    fn revision_number(&self) -> u64 {
        0
    }

    fn increment_height(height: &EvmHeight) -> Option<EvmHeight> {
        height.0.checked_add(1).map(EvmHeight)
    }

    fn sub_height(height: &EvmHeight, n: u64) -> Option<EvmHeight> {
        Some(EvmHeight(height.0.saturating_sub(n).max(1)))
    }

    fn block_time(&self) -> Duration {
        self.config.block_time()
    }

    fn chain_id(&self) -> &Self::ChainId {
        &self.chain_id
    }
}

impl IbcTypes for EthereumChain {
    type ClientState = EvmClientState;
    type ConsensusState = EvmConsensusState;
    type CommitmentProof = EvmCommitmentProof;
    type Packet = EvmPacket;
    type PacketCommitment = EvmPacketCommitment;
    type PacketReceipt = EvmPacketReceipt;
    type Acknowledgement = EvmAcknowledgement;

    fn packet_sequence(packet: &EvmPacket) -> u64 {
        packet.sequence
    }

    fn packet_timeout_timestamp(packet: &EvmPacket) -> u64 {
        packet.timeout_timestamp
    }

    fn packet_source_ports(packet: &EvmPacket) -> Vec<String> {
        packet
            .payloads
            .iter()
            .map(|p| p.source_port.clone())
            .collect()
    }
}

#[async_trait]
impl ClientPayloadBuilder<Self> for EthereumChain {
    type CreateClientPayload = CreateClientPayload;
    type UpdateClientPayload = UpdateClientPayload;

    async fn build_create_client_payload(
        &self,
    ) -> mercury_core::error::Result<CreateClientPayload> {
        match &self.payload_client {
            PayloadClient::Beacon(beacon_api) => {
                self.build_create_client_payload_beacon(beacon_api).await
            }
            PayloadClient::Attested(_) => self.build_create_client_payload_attested().await,
            PayloadClient::Mock => self.build_create_client_payload_mock(),
        }
    }

    async fn build_update_client_payload(
        &self,
        trusted_height: &EvmHeight,
        target_height: &EvmHeight,
        counterparty_client_state: &<Self as IbcTypes>::ClientState,
    ) -> mercury_core::error::Result<UpdateClientPayload> {
        if target_height <= trusted_height {
            eyre::bail!(
                "target height ({}) must be greater than trusted height ({})",
                target_height.0,
                trusted_height.0
            );
        }

        match &self.payload_client {
            PayloadClient::Beacon(beacon_api) => {
                self.build_update_client_payload_beacon(beacon_api, counterparty_client_state)
                    .await
            }
            PayloadClient::Attested(aggregator) => {
                self.build_update_client_payload_attested(aggregator).await
            }
            PayloadClient::Mock => Ok(Self::build_update_client_payload_mock()),
        }
    }
}

impl EthereumChain {
    async fn build_create_client_payload_beacon(
        &self,
        beacon_api: &BeaconApiClient,
    ) -> mercury_core::error::Result<CreateClientPayload> {
        use ethereum_light_client::client_state::ClientState as EthClientState;
        use ethereum_light_client::consensus_state::ConsensusState as EthConsensusState;

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

        let consensus_state = EthConsensusState {
            slot: finalized_slot,
            state_root: finality.data.finalized_header.execution.state_root,
            timestamp: finality.data.finalized_header.execution.timestamp,
            current_sync_committee: bootstrap
                .data
                .current_sync_committee
                .to_summarized_sync_committee(),
            next_sync_committee: None,
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

    async fn build_create_client_payload_attested(
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

    async fn build_update_client_payload_beacon(
        &self,
        beacon_api: &BeaconApiClient,
        counterparty_client_state: &EvmClientState,
    ) -> mercury_core::error::Result<UpdateClientPayload> {
        use ethereum_light_client::client_state::ClientState as EthClientState;
        use ethereum_light_client::header::{ActiveSyncCommittee, Header as EthHeader};

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

        let count = target_period.saturating_sub(trusted_period) + 1;
        let updates = beacon_api
            .light_client_updates(trusted_period, count)
            .await
            .wrap_err("beacon API light_client_updates")?;

        let mut headers = Vec::new();
        let mut current_trusted_slot = trusted_slot;

        for update_response in updates {
            let mut update = update_response.data;
            let update_finalized_slot = update.finalized_header.beacon.slot;

            if update_finalized_slot <= current_trusted_slot {
                continue;
            }

            let update_period =
                eth_client_state.compute_sync_committee_period_at_slot(update_finalized_slot);
            let current_period =
                eth_client_state.compute_sync_committee_period_at_slot(current_trusted_slot);

            let block_root = beacon_api
                .beacon_block_root(&current_trusted_slot.to_string())
                .await
                .wrap_err("beacon API block_root")?;
            let bootstrap = beacon_api
                .light_client_bootstrap(&block_root)
                .await
                .wrap_err("beacon API bootstrap")?;

            let active_sync_committee = if update_period == current_period {
                ActiveSyncCommittee::Current(bootstrap.data.current_sync_committee)
            } else if let Some(next) = update.next_sync_committee.take() {
                ActiveSyncCommittee::Next(next)
            } else {
                ActiveSyncCommittee::Current(bootstrap.data.current_sync_committee)
            };

            let header = EthHeader {
                active_sync_committee,
                consensus_update: update,
                trusted_slot: current_trusted_slot,
            };

            let header_bytes = serde_json::to_vec(&header).wrap_err("serializing beacon header")?;
            headers.push(header_bytes);

            current_trusted_slot = update_finalized_slot;
        }

        Ok(UpdateClientPayload { headers })
    }

    async fn build_update_client_payload_attested(
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
        })
    }

    fn build_create_client_payload_mock(&self) -> mercury_core::error::Result<CreateClientPayload> {
        use ethereum_light_client::client_state::ClientState as EthClientState;

        let state = EthClientState {
            chain_id: self.chain_id.0,
            ibc_contract_address: self.router_address,
            ..EthClientState::default()
        };

        let client_state = serde_json::to_vec(&state).wrap_err("serializing mock client state")?;

        Ok(CreateClientPayload {
            client_state,
            consensus_state: b"{}".to_vec(),
            counterparty_client_id: None,
            counterparty_merkle_prefix: None,
        })
    }

    const fn build_update_client_payload_mock() -> UpdateClientPayload {
        UpdateClientPayload { headers: vec![] }
    }
}

#[must_use]
pub fn to_sol_merkle_prefix(prefix: &mercury_core::MerklePrefix) -> Vec<alloy::primitives::Bytes> {
    prefix
        .0
        .iter()
        .map(|b| alloy::primitives::Bytes::copy_from_slice(b))
        .collect()
}

#[async_trait]
impl ClientMessageBuilder<Self> for EthereumChain {
    type CreateClientPayload = CreateClientPayload;
    type UpdateClientPayload = UpdateClientPayload;

    async fn build_create_client_message(
        &self,
        payload: CreateClientPayload,
    ) -> mercury_core::error::Result<EvmMessage> {
        let merkle_prefix = payload
            .counterparty_merkle_prefix
            .as_ref()
            .map(to_sol_merkle_prefix)
            .unwrap_or_default();

        let call = ICS26Router::addClient_1Call {
            counterpartyInfo: IICS02ClientMsgs::CounterpartyInfo {
                clientId: payload.counterparty_client_id.unwrap_or_default(),
                merklePrefix: merkle_prefix,
            },
            client: self.config.light_client_address()?,
        };

        Ok(EvmMessage {
            to: self.router_address,
            calldata: call.abi_encode(),
            value: U256::ZERO,
        })
    }

    async fn build_update_client_message(
        &self,
        client_id: &EvmClientId,
        payload: UpdateClientPayload,
    ) -> mercury_core::error::Result<UpdateClientOutput<EvmMessage>> {
        #[cfg(feature = "sp1")]
        if let Some(ref sp1) = self.sp1 {
            // Self-relay SP1 path: trusted_consensus_state is not available here.
            // Cross-chain relay uses the bridge crate which provides it.
            return self
                .build_update_client_message_sp1(
                    client_id,
                    payload.headers,
                    None,
                    MembershipProofs::new(),
                    sp1,
                )
                .await;
        }

        let messages = payload
            .headers
            .into_iter()
            .map(|header_bytes| {
                let call = ICS26Router::updateClientCall {
                    clientId: client_id.0.clone(),
                    updateMsg: header_bytes.into(),
                };
                EvmMessage {
                    to: self.router_address,
                    calldata: call.abi_encode(),
                    value: U256::ZERO,
                }
            })
            .collect();

        Ok(UpdateClientOutput::messages_only(messages))
    }

    // Uses `migrateClient` (requires admin role) since EVM has no dedicated
    // `registerCounterparty`. Prefer setting counterparty during `addClient` instead.
    async fn build_register_counterparty_message(
        &self,
        client_id: &EvmClientId,
        counterparty_client_id: &EvmClientId,
        counterparty_merkle_prefix: mercury_core::MerklePrefix,
    ) -> mercury_core::error::Result<EvmMessage> {
        let call = ICS26Router::migrateClientCall {
            clientId: client_id.0.clone(),
            counterpartyInfo: IICS02ClientMsgs::CounterpartyInfo {
                clientId: counterparty_client_id.0.clone(),
                merklePrefix: to_sol_merkle_prefix(&counterparty_merkle_prefix),
            },
            client: self.config.light_client_address()?,
        };

        Ok(EvmMessage {
            to: self.router_address,
            calldata: call.abi_encode(),
            value: U256::ZERO,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increment_height() {
        let h = EvmHeight(100);
        assert_eq!(EthereumChain::increment_height(&h), Some(EvmHeight(101)));
    }

    #[test]
    fn increment_height_overflow() {
        let h = EvmHeight(u64::MAX);
        assert_eq!(EthereumChain::increment_height(&h), None);
    }

    #[test]
    fn sub_height_normal() {
        let h = EvmHeight(10);
        assert_eq!(EthereumChain::sub_height(&h, 3), Some(EvmHeight(7)));
    }

    #[test]
    fn sub_height_clamps_to_one() {
        let h = EvmHeight(5);
        assert_eq!(EthereumChain::sub_height(&h, 100), Some(EvmHeight(1)));
    }

    #[test]
    fn chain_status_extracts() {
        let status = EvmChainStatus {
            height: EvmHeight(42),
            timestamp: EvmTimestamp(1_700_000_000),
        };
        assert_eq!(EthereumChain::chain_status_height(&status).0, 42);
        assert_eq!(
            EthereumChain::chain_status_timestamp_secs(&status),
            1_700_000_000
        );
    }

    #[test]
    fn packet_sequence_extracts() {
        let packet = EvmPacket {
            source_client: "client-0".to_string(),
            dest_client: "client-1".to_string(),
            sequence: 42,
            timeout_timestamp: 0,
            payloads: vec![],
        };
        assert_eq!(<EthereumChain as IbcTypes>::packet_sequence(&packet), 42);
    }

    #[test]
    fn packet_timeout_extracts() {
        let packet = EvmPacket {
            source_client: "client-0".to_string(),
            dest_client: "client-1".to_string(),
            sequence: 1,
            timeout_timestamp: 1_700_000_000,
            payloads: vec![],
        };
        assert_eq!(
            <EthereumChain as IbcTypes>::packet_timeout_timestamp(&packet),
            1_700_000_000
        );
    }
}
