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
use mercury_chain_traits::types::{ChainTypes, IbcTypes, PacketSequence, Port, TimeoutTimestamp};

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
    label: mercury_core::ChainLabel,
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
        let rpc_guard =
            mercury_core::rpc_guard::RpcGuard::new(&config.chain_id.to_string(), rpc_config);

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

        let name = config.chain_name.as_deref().unwrap_or("ethereum");
        let label = mercury_core::ChainLabel::with_id(name, config.chain_id.to_string());
        Ok(Self {
            chain_id: EvmChainId(config.chain_id),
            config,
            router_address,
            provider: Arc::new(provider),
            rpc_guard,
            payload_client,
            #[cfg(feature = "sp1")]
            sp1,
            label,
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

    fn chain_label(&self) -> mercury_core::ChainLabel {
        self.label.clone()
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

    fn packet_sequence(packet: &EvmPacket) -> PacketSequence {
        packet.sequence
    }

    fn packet_timeout_timestamp(packet: &EvmPacket) -> TimeoutTimestamp {
        packet.timeout_timestamp
    }

    fn packet_source_ports(packet: &EvmPacket) -> Vec<Port> {
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
            PayloadClient::Mock => Ok(Self::build_update_client_payload_mock(trusted_height.0)),
        }
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
            // Self-relay path — trusted_consensus_state comes from the bridge crate in cross-chain relay.
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
            sequence: PacketSequence(42),
            timeout_timestamp: TimeoutTimestamp(0),
            payloads: vec![],
        };
        assert_eq!(
            <EthereumChain as IbcTypes>::packet_sequence(&packet),
            PacketSequence(42)
        );
    }

    #[test]
    fn packet_timeout_extracts() {
        let packet = EvmPacket {
            source_client: "client-0".to_string(),
            dest_client: "client-1".to_string(),
            sequence: PacketSequence(1),
            timeout_timestamp: TimeoutTimestamp(1_700_000_000),
            payloads: vec![],
        };
        assert_eq!(
            <EthereumChain as IbcTypes>::packet_timeout_timestamp(&packet),
            TimeoutTimestamp(1_700_000_000)
        );
    }
}
