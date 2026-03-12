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
use mercury_chain_traits::types::{
    HasChainStatusType, HasChainTypes, HasIbcTypes, HasMessageTypes, HasPacketTypes,
    HasRevisionNumber,
};

/// A Cosmos SDK chain connected via RPC and gRPC.
#[derive(Clone, Debug)]
pub struct CosmosChain<S: CosmosSigner> {
    pub config: CosmosChainConfig,
    pub chain_id: ChainId,
    pub rpc_client: HttpClient,
    pub grpc_channel: tonic::transport::Channel,
    pub signer: S,
    pub block_time: Duration,
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
        let chain_id =
            ChainId::new(status.node_info.network.as_str()).map_err(|e| eyre::eyre!("{e}"))?;

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
        })
    }
}

impl<S: CosmosSigner> HasChainTypes for CosmosChain<S> {
    type Height = TmHeight;
    type Timestamp = TmTime;
    type ChainId = ChainId;
    type Event = CosmosEvent;
}

impl<S: CosmosSigner> HasMessageTypes for CosmosChain<S> {
    type Message = CosmosMessage;
    type MessageResponse = CosmosTxResponse;
}

impl<S: CosmosSigner> HasIbcTypes<Self> for CosmosChain<S> {
    type ClientId = ibc::core::host::types::identifiers::ClientId;
    type ClientState = TendermintClientState;
    type ConsensusState = TendermintConsensusState;
    type CommitmentProof = MerkleProof;
}

impl<S: CosmosSigner> HasPacketTypes<Self> for CosmosChain<S> {
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
}

impl<S: CosmosSigner> HasRevisionNumber for CosmosChain<S> {
    fn revision_number(&self) -> u64 {
        self.chain_id.revision_number()
    }
}

impl<S: CosmosSigner> HasChainStatusType for CosmosChain<S> {
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
}
