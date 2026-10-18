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
                ..
            } => PayloadClient::Attested(AggregatorClient::new(
                attestor_endpoints.clone(),
                *quorum_threshold,
            )?),
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
            PayloadClient::Mock => Self::build_update_client_payload_mock(trusted_height.0),
        }
    }

    fn update_payload_proof_height(&self, payload: &UpdateClientPayload) -> Option<EvmHeight> {
        payload.target_execution_height.clone()
    }

    fn update_payload_message_height(&self, payload: &UpdateClientPayload) -> Option<EvmHeight> {
        payload.target_slot.map(EvmHeight)
    }

    fn required_dst_timestamp_secs(&self, payload: &UpdateClientPayload) -> Option<u64> {
        payload.required_dst_timestamp_secs
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

        // Starts empty; populated by migrateClient later.
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

    // EVM has no `registerCounterparty`, so we abuse `migrateClient` (admin-only).
    // Better to set counterparty during `addClient` when possible.
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
pub(crate) mod test_support;

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

    mod builders {
        use super::super::test_support::{LIGHT_CLIENT, ROUTER, offline_chain};
        use super::*;
        use crate::types::EvmPayload;

        fn update_payload(
            target_exec: Option<u64>,
            target_slot: Option<u64>,
        ) -> UpdateClientPayload {
            UpdateClientPayload {
                headers: vec![],
                target_execution_height: target_exec.map(EvmHeight),
                target_slot,
                required_dst_timestamp_secs: Some(99),
            }
        }

        #[test]
        fn to_sol_merkle_prefix_preserves_segments() {
            let prefix = mercury_core::MerklePrefix(vec![b"ibc".to_vec(), vec![]]);
            let sol = to_sol_merkle_prefix(&prefix);
            assert_eq!(sol.len(), 2);
            assert_eq!(sol[0].as_ref(), b"ibc");
            assert!(sol[1].is_empty());
        }

        #[test]
        fn packet_source_ports_lists_every_payload() {
            let payload = |port: &str| EvmPayload {
                source_port: Port(port.to_string()),
                dest_port: Port("dst".to_string()),
                version: String::new(),
                encoding: String::new(),
                value: vec![],
            };
            let packet = EvmPacket {
                source_client: String::new(),
                dest_client: String::new(),
                sequence: PacketSequence(1),
                timeout_timestamp: TimeoutTimestamp(0),
                payloads: vec![payload("transfer"), payload("ica")],
            };
            assert_eq!(
                <EthereumChain as IbcTypes>::packet_source_ports(&packet),
                vec![Port("transfer".to_string()), Port("ica".to_string())]
            );
        }

        #[test]
        fn payload_client_debug_hides_beacon_client() {
            let beacon =
                PayloadClient::Beacon(Arc::new(BeaconApiClient::new("http://secret".to_string())));
            assert_eq!(format!("{beacon:?}"), "Beacon");
            assert_eq!(format!("{:?}", PayloadClient::Mock), "Mock");
        }

        #[test]
        fn payload_heights_come_from_payload() {
            let chain = offline_chain();
            let payload = update_payload(Some(500), Some(1000));
            assert_eq!(
                chain.update_payload_proof_height(&payload),
                Some(EvmHeight(500))
            );
            assert_eq!(
                chain.update_payload_message_height(&payload),
                Some(EvmHeight(1000))
            );
            assert_eq!(chain.required_dst_timestamp_secs(&payload), Some(99));

            let empty = update_payload(None, None);
            assert_eq!(chain.update_payload_proof_height(&empty), None);
            assert_eq!(chain.update_payload_message_height(&empty), None);
        }

        #[tokio::test]
        async fn update_payload_rejects_non_increasing_target() {
            let chain = offline_chain();
            let cs = EvmClientState(vec![]);
            for target in [5, 10] {
                let err = chain
                    .build_update_client_payload(&EvmHeight(10), &EvmHeight(target), &cs)
                    .await
                    .unwrap_err();
                assert!(err.to_string().contains("must be greater"), "{err}");
            }
        }

        #[tokio::test]
        async fn mock_mode_dispatches_to_mock_builders() {
            let chain = offline_chain();
            let create = chain.build_create_client_payload().await.unwrap();
            assert_ne!(create.client_state, Vec::<u8>::new());

            let update = chain
                .build_update_client_payload(
                    &EvmHeight(10),
                    &EvmHeight(11),
                    &EvmClientState(vec![]),
                )
                .await
                .unwrap();
            assert_eq!(update.headers.len(), 1);
        }

        #[tokio::test]
        async fn create_client_message_encodes_counterparty() {
            let chain = offline_chain();
            let payload = CreateClientPayload {
                client_state: vec![],
                consensus_state: vec![],
                counterparty_client_id: Some("08-wasm-3".to_string()),
                counterparty_merkle_prefix: Some(mercury_core::MerklePrefix::ibc_default()),
            };
            let msg = chain.build_create_client_message(payload).await.unwrap();
            assert_eq!(msg.to, ROUTER);
            assert_eq!(msg.value, U256::ZERO);
            let call = ICS26Router::addClient_1Call::abi_decode(&msg.calldata).unwrap();
            assert_eq!(call.client, LIGHT_CLIENT);
            assert_eq!(call.counterpartyInfo.clientId, "08-wasm-3");
            assert_eq!(call.counterpartyInfo.merklePrefix.len(), 2);
        }

        #[tokio::test]
        async fn create_client_message_defaults_missing_counterparty() {
            let chain = offline_chain();
            let payload = CreateClientPayload {
                client_state: vec![],
                consensus_state: vec![],
                counterparty_client_id: None,
                counterparty_merkle_prefix: None,
            };
            let msg = chain.build_create_client_message(payload).await.unwrap();
            let call = ICS26Router::addClient_1Call::abi_decode(&msg.calldata).unwrap();
            assert_eq!(call.counterpartyInfo.clientId, "");
            assert_eq!(
                call.counterpartyInfo.merklePrefix,
                Vec::<alloy::primitives::Bytes>::new()
            );
        }

        #[tokio::test]
        async fn create_client_message_requires_light_client_address() {
            let mut chain = offline_chain();
            chain.config.light_client_address = None;
            let payload = CreateClientPayload {
                client_state: vec![],
                consensus_state: vec![],
                counterparty_client_id: None,
                counterparty_merkle_prefix: None,
            };
            let err = chain
                .build_create_client_message(payload)
                .await
                .unwrap_err();
            assert!(err.to_string().contains("light_client_address"), "{err}");
        }

        #[tokio::test]
        async fn update_client_message_emits_one_call_per_header() {
            let chain = offline_chain();
            let payload = UpdateClientPayload {
                headers: vec![vec![1, 2], vec![3]],
                target_execution_height: None,
                target_slot: None,
                required_dst_timestamp_secs: None,
            };
            let client_id = EvmClientId("client-7".to_string());
            let output = chain
                .build_update_client_message(&client_id, payload)
                .await
                .unwrap();
            assert!(output.membership_proof.is_none());
            assert_eq!(output.messages.len(), 2);
            let decoded: Vec<_> = output
                .messages
                .iter()
                .map(|m| {
                    assert_eq!(m.to, ROUTER);
                    ICS26Router::updateClientCall::abi_decode(&m.calldata).unwrap()
                })
                .collect();
            assert_eq!(decoded[0].clientId, "client-7");
            assert_eq!(decoded[0].updateMsg.as_ref(), &[1, 2]);
            assert_eq!(decoded[1].updateMsg.as_ref(), &[3]);
        }

        #[tokio::test]
        async fn register_counterparty_uses_migrate_client() {
            let chain = offline_chain();
            let msg = chain
                .build_register_counterparty_message(
                    &EvmClientId("client-0".to_string()),
                    &EvmClientId("08-wasm-1".to_string()),
                    mercury_core::MerklePrefix(vec![b"ibc".to_vec()]),
                )
                .await
                .unwrap();
            assert_eq!(msg.to, ROUTER);
            let call = ICS26Router::migrateClientCall::abi_decode(&msg.calldata).unwrap();
            assert_eq!(call.clientId, "client-0");
            assert_eq!(call.counterpartyInfo.clientId, "08-wasm-1");
            assert_eq!(call.counterpartyInfo.merklePrefix[0].as_ref(), b"ibc");
            assert_eq!(call.client, LIGHT_CLIENT);
        }

        #[tokio::test]
        async fn register_counterparty_requires_light_client_address() {
            let mut chain = offline_chain();
            chain.config.light_client_address = None;
            let err = chain
                .build_register_counterparty_message(
                    &EvmClientId("client-0".to_string()),
                    &EvmClientId("08-wasm-1".to_string()),
                    mercury_core::MerklePrefix::ibc_default(),
                )
                .await
                .unwrap_err();
            assert!(err.to_string().contains("light_client_address"), "{err}");
        }
    }

    mod connect {
        use serde_json::json;

        use super::super::test_support::{CHAIN_ID, ROUTER, spawn_rpc, test_config};
        use super::*;

        fn signer() -> alloy::signers::local::PrivateKeySigner {
            alloy::signers::local::PrivateKeySigner::from_bytes(
                &alloy::primitives::B256::repeat_byte(1),
            )
            .unwrap()
        }

        async fn node(chain_id: u64, syncing: bool) -> String {
            spawn_rpc(move |method, _| match method {
                "eth_chainId" => Ok(json!(format!("{chain_id:#x}"))),
                "eth_syncing" if syncing => Ok(json!({
                    "startingBlock": "0x0",
                    "currentBlock": "0x1",
                    "highestBlock": "0x10",
                })),
                "eth_syncing" => Ok(json!(false)),
                other => Err(format!("unexpected {other}")),
            })
            .await
        }

        #[tokio::test]
        async fn new_selects_payload_client_from_mode() {
            let url = node(CHAIN_ID, false).await;
            let modes = [
                ClientPayloadMode::Mock,
                ClientPayloadMode::Beacon {
                    beacon_api_url: "http://beacon".to_string(),
                },
                ClientPayloadMode::Attested {
                    attestor_endpoints: vec!["http://a".to_string()],
                    attestor_addresses: vec![],
                    quorum_threshold: 1,
                },
            ];
            let mut kinds = Vec::new();
            for mode in modes {
                let chain = EthereumChain::new(test_config(&url, mode), signer())
                    .await
                    .unwrap();
                assert_eq!(chain.chain_id, EvmChainId(CHAIN_ID));
                assert_eq!(chain.router_address, ROUTER);
                assert_eq!(chain.revision_number(), 0);
                assert_eq!(chain.block_time(), Duration::from_secs(1));
                assert_eq!(
                    chain.chain_label().to_string(),
                    format!("ethereum/{CHAIN_ID}")
                );
                kinds.push(format!("{:?}", chain.payload_client));
            }
            assert_eq!(kinds[0], "Mock");
            assert_eq!(kinds[1], "Beacon");
            assert!(kinds[2].starts_with("Attested"), "{}", kinds[2]);
        }

        #[tokio::test]
        async fn new_uses_configured_chain_name_in_label() {
            let url = node(CHAIN_ID, false).await;
            let mut config = test_config(&url, ClientPayloadMode::Mock);
            config.chain_name = Some("sepolia".to_string());
            let chain = EthereumChain::new(config, signer()).await.unwrap();
            assert!(chain.chain_label().to_string().contains("sepolia"));
        }

        #[tokio::test]
        async fn new_rejects_chain_id_mismatch() {
            let url = node(1, false).await;
            let err = EthereumChain::new(test_config(&url, ClientPayloadMode::Mock), signer())
                .await
                .unwrap_err();
            assert!(err.to_string().contains("chain_id mismatch"), "{err}");
        }

        #[tokio::test]
        async fn new_rejects_syncing_node() {
            let url = node(CHAIN_ID, true).await;
            let err = EthereumChain::new(test_config(&url, ClientPayloadMode::Mock), signer())
                .await
                .unwrap_err();
            assert!(err.to_string().contains("still syncing"), "{err}");
        }

        #[tokio::test]
        async fn new_rejects_bad_rpc_url_and_router() {
            let err =
                EthereumChain::new(test_config("not a url", ClientPayloadMode::Mock), signer())
                    .await
                    .unwrap_err();
            assert!(err.to_string().contains("parsing RPC URL"), "{err}");

            let url = node(CHAIN_ID, false).await;
            let mut config = test_config(&url, ClientPayloadMode::Mock);
            config.ics26_router = "0xnope".to_string();
            let err = EthereumChain::new(config, signer()).await.unwrap_err();
            assert!(err.to_string().contains("ics26_router"), "{err}");
        }
    }
}
