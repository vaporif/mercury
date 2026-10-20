use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ibc::core::host::types::identifiers::ChainId;
use tendermint::Time as TmTime;
use tendermint::block::Height as TmHeight;
use tendermint::node::info::TxIndexStatus;
use tendermint_rpc::HttpClient;
use tracing::{info, warn};

use crate::client_types::{CosmosClientState, CosmosConsensusState};
use crate::config::CosmosChainConfig;
use crate::keys::CosmosSigner;
use crate::types::{
    CosmosChainStatus, CosmosEvent, CosmosMessage, CosmosPacket, CosmosTxResponse, MerkleProof,
    PacketAcknowledgement, PacketCommitment, PacketReceipt,
};
use mercury_chain_traits::types::{ChainTypes, IbcTypes, PacketSequence, Port, TimeoutTimestamp};

#[derive(Clone, Debug)]
pub struct CosmosChain<S: CosmosSigner> {
    pub config: CosmosChainConfig,
    pub chain_id: ChainId,
    pub rpc_client: HttpClient,
    pub grpc_channel: tonic::transport::Channel,
    pub signer: S,
    pub block_time: Duration,
    pub rpc_guard: mercury_core::rpc_guard::RpcGuard,
    pub dynamic_gas_backend: Arc<OnceLock<crate::gas::DynamicGasBackend>>,
    /// Lives here (rather than in a relay worker) because gas price resolution
    /// happens inside `estimate_fee_with_nonce`, which only `CosmosChain` owns.
    pub gas_metrics: mercury_telemetry::recorder::GasMetrics,
    label: mercury_core::ChainLabel,
}

impl<S: CosmosSigner> CosmosChain<S> {
    pub async fn new(config: CosmosChainConfig, signer: S) -> mercury_core::error::Result<Self> {
        use mercury_core::error::WrapErr;
        use tendermint_rpc::Client;

        let rpc_client =
            HttpClient::new(config.rpc_addr.as_str()).wrap_err("creating RPC client")?;

        let rpc_config = config.rpc_config();
        rpc_config.validate().wrap_err("invalid RPC config")?;
        let rpc_guard = mercury_core::rpc_guard::RpcGuard::new(&config.chain_id, rpc_config);

        let status = rpc_guard
            .guarded(|| async { rpc_client.status().await.map_err(Into::into) })
            .await
            .wrap_err("querying chain status")?;
        let chain_id = ChainId::new(status.node_info.network.as_str())
            .map_err(|e| eyre::eyre!("parsing chain ID: {e}"))?;

        if status.node_info.other.tx_index == TxIndexStatus::Off {
            eyre::bail!(
                "chain '{}': node has tx indexing disabled — mercury requires tx indexing for event queries",
                config.chain_id,
            );
        }

        if status.sync_info.catching_up {
            eyre::bail!(
                "chain '{}': node is still syncing — wait for it to catch up before starting the relayer",
                config.chain_id,
            );
        }

        let grpc_endpoint = tonic::transport::Channel::from_shared(config.grpc_addr.clone())
            .wrap_err("parsing gRPC address")?;
        let grpc_endpoint = if config.grpc_addr.starts_with("https") {
            grpc_endpoint
                .tls_config(tonic::transport::ClientTlsConfig::new().with_native_roots())
                .wrap_err("configuring TLS")?
        } else {
            grpc_endpoint
        };
        // Transport timeout is rpc_timeout + 5s safety buffer.
        // RpcGuard fires first for clean error reporting; this is a safety net.
        let transport_timeout = std::time::Duration::from_secs(config.rpc_timeout_secs + 5);
        let grpc_channel = grpc_endpoint
            .timeout(transport_timeout)
            .connect()
            .await
            .wrap_err("connecting to gRPC")?;

        check_min_gas_price(grpc_channel.clone(), &config).await;
        info!(chain_id = %chain_id, "cosmos chain initialized");

        let name = config.chain_name.as_deref().unwrap_or("cosmos");
        let label = mercury_core::ChainLabel::with_id(name, chain_id.to_string());
        let gas_metrics = mercury_telemetry::recorder::GasMetrics::new(&label);
        Ok(Self {
            block_time: config.block_time,
            config,
            chain_id,
            rpc_client,
            grpc_channel,
            signer,
            rpc_guard,
            dynamic_gas_backend: Arc::new(OnceLock::new()),
            gas_metrics,
            label,
        })
    }
}

impl<S: CosmosSigner> ChainTypes for CosmosChain<S> {
    type Height = TmHeight;
    type Timestamp = TmTime;
    type ChainId = ChainId;
    type ClientId = ibc::core::host::types::identifiers::ClientId;
    type Event = CosmosEvent;
    type Message = CosmosMessage;
    type MessageResponse = CosmosTxResponse;
    type ChainStatus = CosmosChainStatus;

    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height {
        &status.height
    }

    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp {
        &status.timestamp
    }

    fn chain_status_timestamp_secs(status: &Self::ChainStatus) -> u64 {
        status.timestamp.unix_timestamp().try_into().unwrap_or(0)
    }

    fn revision_number(&self) -> u64 {
        self.chain_id.revision_number()
    }

    fn increment_height(height: &TmHeight) -> Option<TmHeight> {
        height
            .value()
            .checked_add(1)
            .and_then(|v| TmHeight::try_from(v).ok())
    }

    fn sub_height(height: &TmHeight, n: u64) -> Option<TmHeight> {
        let val = height.value().saturating_sub(n).max(1);
        TmHeight::try_from(val).ok()
    }

    fn block_time(&self) -> Duration {
        self.block_time
    }

    fn max_clock_drift(&self) -> Duration {
        self.config
            .max_clock_drift
            .unwrap_or(super::builders::DEFAULT_MAX_CLOCK_DRIFT)
    }

    fn chain_id(&self) -> &Self::ChainId {
        &self.chain_id
    }

    fn chain_label(&self) -> mercury_core::ChainLabel {
        self.label.clone()
    }
}

impl<S: CosmosSigner> IbcTypes for CosmosChain<S> {
    type ClientState = CosmosClientState;
    type ConsensusState = CosmosConsensusState;
    type CommitmentProof = MerkleProof;
    type Packet = CosmosPacket;
    type PacketCommitment = PacketCommitment;
    type PacketReceipt = PacketReceipt;
    type Acknowledgement = PacketAcknowledgement;

    fn packet_sequence(packet: &CosmosPacket) -> PacketSequence {
        packet.sequence
    }

    fn packet_timeout_timestamp(packet: &CosmosPacket) -> TimeoutTimestamp {
        packet.timeout_timestamp
    }

    fn packet_source_ports(packet: &CosmosPacket) -> Vec<Port> {
        packet
            .payloads
            .iter()
            .map(|p| p.source_port.clone())
            .collect()
    }
}

/// Parses a Cosmos SDK `DecCoin` string like `"0.025uatom"` into (amount, denom).
fn parse_dec_coin(s: &str) -> Option<(f64, &str)> {
    let pos = s.find(|c: char| c.is_alphabetic())?;
    let (amount, denom) = s.split_at(pos);
    Some((amount.parse().ok()?, denom))
}

/// Non-fatal: some nodes don't expose this endpoint.
async fn check_min_gas_price(channel: tonic::transport::Channel, config: &CosmosChainConfig) {
    use ibc_proto::cosmos::base::node::v1beta1::{
        ConfigRequest, service_client::ServiceClient as NodeServiceClient,
    };

    let Ok(response) = NodeServiceClient::new(channel)
        .config(ConfigRequest {})
        .await
    else {
        return;
    };

    // minimum_gas_price is comma-separated, e.g. "0.025uatom,0.001uosmo"
    let min_gas_prices = response.into_inner().minimum_gas_price;
    let node_min = min_gas_prices
        .split(',')
        .filter_map(|e| parse_dec_coin(e.trim()))
        .find(|(_, denom)| *denom == config.gas_price.denom);

    if let Some((node_min, denom)) = node_min
        && config.gas_price.amount < node_min
    {
        warn!(
            chain_id = %config.chain_id,
            configured = config.gas_price.amount,
            node_minimum = node_min,
            denom,
            "configured gas price is below node's minimum — transactions may be rejected",
        );
    }
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Secp256k1KeyPair;
    use mercury_chain_traits::types::{ChainTypes, IbcTypes, PacketSequence, TimeoutTimestamp};

    type TestChain = CosmosChain<Secp256k1KeyPair>;

    #[test]
    fn increment_height_normal() {
        let h = TmHeight::try_from(10u64).unwrap();
        let next = TestChain::increment_height(&h).unwrap();
        assert_eq!(next.value(), 11);
    }

    #[test]
    fn increment_height_one() {
        let h = TmHeight::try_from(1u64).unwrap();
        let next = TestChain::increment_height(&h).unwrap();
        assert_eq!(next.value(), 2);
    }

    #[test]
    fn chain_status_height_extracts() {
        let status = CosmosChainStatus {
            height: TmHeight::try_from(42u64).unwrap(),
            timestamp: TmTime::unix_epoch(),
        };
        let h = TestChain::chain_status_height(&status);
        assert_eq!(h.value(), 42);
    }

    #[test]
    fn chain_status_timestamp_secs_extracts() {
        let ts = (TmTime::unix_epoch() + Duration::from_secs(1000)).unwrap();
        let status = CosmosChainStatus {
            height: TmHeight::try_from(1u64).unwrap(),
            timestamp: ts,
        };
        let secs = TestChain::chain_status_timestamp_secs(&status);
        assert_eq!(secs, 1000);
    }

    #[test]
    fn packet_sequence_extracts() {
        use crate::types::RawClientId;
        let packet = CosmosPacket {
            source_client_id: RawClientId("07-tendermint-0".into()),
            dest_client_id: RawClientId("07-tendermint-1".into()),
            sequence: PacketSequence(99),
            timeout_timestamp: TimeoutTimestamp(0),
            payloads: vec![],
        };
        let seq = TestChain::packet_sequence(&packet);
        assert_eq!(seq, PacketSequence(99));
    }

    #[test]
    fn sub_height_normal() {
        let h = TmHeight::try_from(10u64).unwrap();
        let result = TestChain::sub_height(&h, 3).unwrap();
        assert_eq!(result.value(), 7);
    }

    #[test]
    fn sub_height_clamps_to_one() {
        let h = TmHeight::try_from(5u64).unwrap();
        let result = TestChain::sub_height(&h, 100).unwrap();
        assert_eq!(result.value(), 1);
    }

    #[test]
    fn sub_height_zero() {
        let h = TmHeight::try_from(10u64).unwrap();
        let result = TestChain::sub_height(&h, 0).unwrap();
        assert_eq!(result.value(), 10);
    }

    #[test]
    fn packet_timeout_timestamp_extracts() {
        use crate::types::RawClientId;
        let packet = CosmosPacket {
            source_client_id: RawClientId("07-tendermint-0".into()),
            dest_client_id: RawClientId("07-tendermint-1".into()),
            sequence: PacketSequence(1),
            timeout_timestamp: TimeoutTimestamp(1_700_000_000),
            payloads: vec![],
        };
        let ts = TestChain::packet_timeout_timestamp(&packet);
        assert_eq!(ts, TimeoutTimestamp(1_700_000_000));
    }

    #[test]
    fn increment_height_at_max_returns_none() {
        let max = TmHeight::try_from(u64::try_from(i64::MAX).unwrap()).unwrap();
        assert!(TestChain::increment_height(&max).is_none());
    }

    #[test]
    fn chain_status_timestamp_secs_pre_epoch_is_zero() {
        let status = CosmosChainStatus {
            height: TmHeight::try_from(1u64).unwrap(),
            timestamp: TmTime::from_unix_timestamp(-100, 0).unwrap(),
        };
        assert_eq!(TestChain::chain_status_timestamp_secs(&status), 0);
    }

    #[test]
    fn packet_source_ports_preserves_payload_order() {
        use crate::types::{PacketPayload, RawClientId};
        let payload = |port: &str| PacketPayload {
            source_port: Port(port.to_string()),
            dest_port: Port("dest".to_string()),
            version: String::new(),
            encoding: String::new(),
            data: vec![],
        };
        let packet = CosmosPacket {
            source_client_id: RawClientId("07-tendermint-0".into()),
            dest_client_id: RawClientId("07-tendermint-1".into()),
            sequence: PacketSequence(1),
            timeout_timestamp: TimeoutTimestamp(0),
            payloads: vec![payload("transfer"), payload("ica")],
        };
        assert_eq!(
            TestChain::packet_source_ports(&packet),
            vec![Port("transfer".to_string()), Port("ica".to_string())]
        );
    }

    #[test]
    fn parse_dec_coin_splits_amount_and_denom() {
        let (amount, denom) = parse_dec_coin("0.025uatom").unwrap();
        assert!((amount - 0.025).abs() < f64::EPSILON);
        assert_eq!(denom, "uatom");
    }

    #[test]
    fn parse_dec_coin_sdk_precision_and_ibc_denom() {
        let (amount, denom) = parse_dec_coin("0.100000000000000000ibc/27394FB092D2ECCD").unwrap();
        assert!((amount - 0.1).abs() < f64::EPSILON);
        assert_eq!(denom, "ibc/27394FB092D2ECCD");
    }

    #[test]
    fn parse_dec_coin_rejects_missing_parts() {
        assert!(parse_dec_coin("uatom").is_none(), "no amount");
        assert!(parse_dec_coin("0.025").is_none(), "no denom");
        assert!(parse_dec_coin("").is_none());
        assert!(parse_dec_coin("1.2.3uatom").is_none(), "malformed amount");
    }

    #[tokio::test]
    async fn chain_accessors_reflect_config() {
        let mut config = test_support::test_config();
        let chain = test_support::test_chain_with(config.clone());
        assert_eq!(chain.revision_number(), 1);
        assert_eq!(chain.block_time(), config.block_time);
        assert_eq!(
            chain.max_clock_drift(),
            super::super::builders::DEFAULT_MAX_CLOCK_DRIFT
        );
        assert_eq!(chain.chain_id().as_str(), "testchain-1");
        assert!(chain.chain_label().to_string().contains("testchain-1"));

        config.max_clock_drift = Some(Duration::from_secs(7));
        let chain = test_support::test_chain_with(config);
        assert_eq!(chain.max_clock_drift(), Duration::from_secs(7));
    }

    mod connect {
        use super::*;
        use crate::chain::test_support::grpc::{
            GrpcReply, MockGrpc, reply, spawn_grpc, unimplemented,
        };
        use crate::chain::test_support::rpc::{MockRpc, Reply, spawn_rpc};
        use crate::chain::test_support::tm::status_json;
        use crate::chain::test_support::{UNREACHABLE_ADDR, mock_config, test_signer};
        use ibc_proto::cosmos::base::node::v1beta1::ConfigResponse;

        const NODE_CONFIG: &str = "/cosmos.base.node.v1beta1.Service/Config";

        async fn node(catching_up: bool, tx_index_on: bool) -> MockRpc {
            spawn_rpc(move |_| Reply::Result(status_json(500, catching_up, tx_index_on))).await
        }

        async fn grpc_with_min_gas(min: &'static str) -> MockGrpc {
            spawn_grpc(move |call| match call.path.as_str() {
                NODE_CONFIG => reply(&ConfigResponse {
                    minimum_gas_price: min.into(),
                    ..Default::default()
                }),
                _ => unimplemented(call),
            })
            .await
        }

        async fn connect(config: CosmosChainConfig) -> mercury_core::error::Result<TestChain> {
            CosmosChain::new(config, test_signer()).await
        }

        #[tokio::test]
        async fn new_takes_chain_id_from_node_and_checks_min_gas_price() {
            let rpc = node(false, true).await;
            // configured 0.025uatom is below the node's 0.05uatom: warns, still connects
            let grpc = grpc_with_min_gas("0.001uosmo, 0.05uatom").await;
            let mut config = mock_config(&rpc.url, &grpc.url);
            config.chain_name = Some("hub".into());
            let chain = connect(config).await.unwrap();

            assert_eq!(chain.chain_id().as_str(), "testchain-1");
            assert_eq!(chain.revision_number(), 1);
            let label = chain.chain_label().to_string();
            assert!(
                label.contains("hub") && label.contains("testchain-1"),
                "{label}"
            );
            assert_eq!(grpc.calls_to(NODE_CONFIG).len(), 1);
            assert_eq!(rpc.calls()[0].method, "status");
        }

        #[tokio::test]
        async fn new_tolerates_node_without_config_service() {
            let rpc = node(false, true).await;
            let grpc = spawn_grpc(unimplemented).await;
            connect(mock_config(&rpc.url, &grpc.url)).await.unwrap();
            let grpc = spawn_grpc(|_| GrpcReply::Err(tonic::Status::internal("x"))).await;
            connect(mock_config(&rpc.url, &grpc.url)).await.unwrap();
        }

        #[tokio::test]
        async fn new_rejects_node_without_tx_index() {
            let rpc = node(false, false).await;
            let grpc = grpc_with_min_gas("").await;
            let err = connect(mock_config(&rpc.url, &grpc.url)).await.unwrap_err();
            assert!(err.to_string().contains("tx indexing disabled"), "{err:#}");
            assert!(grpc.calls().is_empty(), "fails before touching gRPC");
        }

        #[tokio::test]
        async fn new_rejects_syncing_node() {
            let rpc = node(true, true).await;
            let err = connect(mock_config(&rpc.url, UNREACHABLE_ADDR))
                .await
                .unwrap_err();
            assert!(err.to_string().contains("still syncing"), "{err:#}");
        }

        #[tokio::test]
        async fn new_rejects_invalid_rpc_config_before_any_call() {
            let rpc = node(false, true).await;
            let mut config = mock_config(&rpc.url, UNREACHABLE_ADDR);
            config.rpc_rate_limit = 0;
            let err = connect(config).await.unwrap_err();
            assert!(format!("{err:#}").contains("invalid RPC config"), "{err:#}");
            assert!(rpc.calls().is_empty());
        }

        #[tokio::test]
        async fn new_with_https_grpc_errors_instead_of_panicking() {
            let rpc = node(false, true).await;
            // nothing listens on port 1: TLS setup must succeed and the connect fail
            let config = mock_config(&rpc.url, "https://127.0.0.1:1");
            let outcome = tokio::spawn(connect(config)).await;
            match outcome {
                Ok(Err(err)) => {
                    assert!(format!("{err:#}").contains("connecting to gRPC"), "{err:#}");
                }
                Ok(Ok(_)) => panic!("connected to a closed port"),
                Err(join) => {
                    let panic = join.into_panic();
                    let msg = panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(ToString::to_string))
                        .unwrap_or_default();
                    panic!("CosmosChain::new panicked: {msg}");
                }
            }
        }

        #[tokio::test]
        async fn new_surfaces_status_and_grpc_connect_failures() {
            let rpc = spawn_rpc(|_| Reply::Error("down".into())).await;
            let err = connect(mock_config(&rpc.url, UNREACHABLE_ADDR))
                .await
                .unwrap_err();
            assert!(
                format!("{err:#}").contains("querying chain status"),
                "{err:#}"
            );

            let rpc = node(false, true).await;
            let err = connect(mock_config(&rpc.url, UNREACHABLE_ADDR))
                .await
                .unwrap_err();
            assert!(format!("{err:#}").contains("connecting to gRPC"), "{err:#}");

            let err = connect(mock_config(&rpc.url, "http://bad host"))
                .await
                .unwrap_err();
            assert!(
                format!("{err:#}").contains("parsing gRPC address"),
                "{err:#}"
            );
        }
    }
}
