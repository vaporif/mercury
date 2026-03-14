use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ibc::core::host::types::identifiers::ChainId;
use ibc_client_tendermint::types::ClientState as TendermintClientState;
use ibc_client_tendermint::types::ConsensusState as TendermintConsensusState;
use tendermint::Time as TmTime;
use tendermint::block::Height as TmHeight;
use tendermint_rpc::HttpClient;

use crate::config::CosmosChainConfig;
use crate::keys::CosmosSigner;
use crate::types::{
    CosmosChainStatus, CosmosEvent, CosmosMessage, CosmosPacket, CosmosTxResponse, MerkleProof,
    PacketAcknowledgement, PacketCommitment, PacketReceipt,
};
use mercury_chain_traits::types::{ChainTypes, IbcTypes};

/// A Cosmos SDK chain connected via RPC and gRPC.
#[derive(Clone, Debug)]
pub struct CosmosChain<S: CosmosSigner> {
    pub config: CosmosChainConfig,
    pub chain_id: ChainId,
    pub rpc_client: HttpClient,
    pub grpc_channel: tonic::transport::Channel,
    pub signer: S,
    pub block_time: Duration,
    pub dynamic_gas_backend: Arc<OnceLock<crate::gas::DynamicGasBackend>>,
}

impl<S: CosmosSigner> CosmosChain<S> {
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
}

impl<S: CosmosSigner> IbcTypes for CosmosChain<S> {
    type ClientState = TendermintClientState;
    type ConsensusState = TendermintConsensusState;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Secp256k1KeyPair;
    use mercury_chain_traits::types::{ChainTypes, IbcTypes};

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
        use ibc::core::host::types::identifiers::ClientId;
        let packet = CosmosPacket {
            source_client_id: ClientId::new("07-tendermint", 0).unwrap(),
            dest_client_id: ClientId::new("07-tendermint", 1).unwrap(),
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
        use ibc::core::host::types::identifiers::ClientId;
        let packet = CosmosPacket {
            source_client_id: ClientId::new("07-tendermint", 0).unwrap(),
            dest_client_id: ClientId::new("07-tendermint", 1).unwrap(),
            sequence: 1,
            timeout_timestamp: 1_700_000_000,
            payloads: vec![],
        };
        let ts = TestChain::packet_timeout_timestamp(&packet);
        assert_eq!(ts, 1_700_000_000);
    }
}
