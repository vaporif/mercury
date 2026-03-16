use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ibc::core::host::types::identifiers::ChainId;
use tendermint::Time as TmTime;
use tendermint::block::Height as TmHeight;
use tendermint::node::info::TxIndexStatus;
use tendermint_rpc::HttpClient;
use tracing::warn;

use crate::client_types::{CosmosClientState, CosmosConsensusState};
use crate::config::CosmosChainConfig;
use crate::keys::CosmosSigner;
use crate::types::{
    CosmosChainStatus, CosmosEvent, CosmosMessage, CosmosPacket, CosmosTxResponse, MerkleProof,
    PacketAcknowledgement, PacketCommitment, PacketReceipt,
};
use mercury_chain_traits::types::{ChainTypes, IbcTypes};

/// A Cosmos SDK chain connected via RPC and gRPC.
#[derive(Clone, Debug)]
pub struct CosmosChainInner<S: CosmosSigner> {
    pub config: CosmosChainConfig,
    pub chain_id: ChainId,
    pub rpc_client: HttpClient,
    pub grpc_channel: tonic::transport::Channel,
    pub signer: S,
    pub block_time: Duration,
    pub dynamic_gas_backend: Arc<OnceLock<crate::gas::DynamicGasBackend>>,
}

impl<S: CosmosSigner> CosmosChainInner<S> {
    pub async fn new(config: CosmosChainConfig, signer: S) -> mercury_core::error::Result<Self> {
        use mercury_core::error::WrapErr;
        use tendermint_rpc::Client;

        let rpc_client =
            HttpClient::new(config.rpc_addr.as_str()).wrap_err("creating RPC client")?;

        let status = rpc_client
            .status()
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
        let grpc_channel = grpc_endpoint
            .connect()
            .await
            .wrap_err("connecting to gRPC")?;

        check_min_gas_price(grpc_channel.clone(), &config).await;

        Ok(Self {
            block_time: config.block_time,
            config,
            chain_id,
            rpc_client,
            grpc_channel,
            signer,
            dynamic_gas_backend: Arc::new(OnceLock::new()),
        })
    }
}

impl<S: CosmosSigner> ChainTypes for CosmosChainInner<S> {
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
}

impl<S: CosmosSigner> IbcTypes for CosmosChainInner<S> {
    type ClientState = CosmosClientState;
    type ConsensusState = CosmosConsensusState;
    type CommitmentProof = MerkleProof;
    type Packet = CosmosPacket;
    type PacketCommitment = PacketCommitment;
    type PacketReceipt = PacketReceipt;
    type Acknowledgement = PacketAcknowledgement;

    fn packet_sequence(packet: &CosmosPacket) -> u64 {
        packet.sequence
    }

    fn packet_timeout_timestamp(packet: &CosmosPacket) -> u64 {
        packet.timeout_timestamp
    }

    fn packet_source_ports(packet: &CosmosPacket) -> Vec<String> {
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
mod tests {
    use super::*;
    use crate::keys::Secp256k1KeyPair;
    use mercury_chain_traits::types::{ChainTypes, IbcTypes};

    type TestChain = CosmosChainInner<Secp256k1KeyPair>;

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
            sequence: 99,
            timeout_timestamp: 0,
            payloads: vec![],
        };
        let seq = TestChain::packet_sequence(&packet);
        assert_eq!(seq, 99);
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
            sequence: 1,
            timeout_timestamp: 1_700_000_000,
            payloads: vec![],
        };
        let ts = TestChain::packet_timeout_timestamp(&packet);
        assert_eq!(ts, 1_700_000_000);
    }
}
