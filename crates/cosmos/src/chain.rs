use std::sync::Arc;
use std::time::Duration;

use ibc::core::host::types::identifiers::ChainId;
use ibc_client_tendermint::types::ClientState as TendermintClientState;
use ibc_client_tendermint::types::ConsensusState as TendermintConsensusState;
use tendermint::Time as TmTime;
use tendermint::block::Height as TmHeight;
use tendermint_rpc::HttpClient;
use tokio::sync::Mutex;

use crate::config::CosmosChainConfig;
use crate::keys::Secp256k1KeyPair;
use crate::tx::CosmosNonce;
use crate::types::{
    CosmosChainStatus, CosmosEvent, CosmosMessage, CosmosPacket, CosmosTxResponse, MerkleProof,
    PacketAcknowledgement, PacketCommitment, PacketReceipt,
};
use mercury_chain_traits::types::{
    HasChainStatusType, HasChainTypes, HasIbcTypes, HasMessageTypes, HasPacketTypes,
};

#[derive(Clone)]
pub struct CosmosChain {
    pub config: CosmosChainConfig,
    pub chain_id: ChainId,
    pub rpc_client: HttpClient,
    pub grpc_channel: tonic::transport::Channel,
    pub signer: Secp256k1KeyPair,
    pub block_time: Duration,
    pub nonce_mutex: Arc<Mutex<Option<CosmosNonce>>>,
}

impl CosmosChain {
    pub async fn new(config: CosmosChainConfig) -> mercury_core::error::Result<Self> {
        use mercury_core::error::Error;
        use tendermint_rpc::Client;

        let rpc_client = HttpClient::new(config.rpc_addr.as_str()).map_err(Error::report)?;

        let status = rpc_client.status().await.map_err(Error::report)?;
        let chain_id = ChainId::new(status.node_info.network.as_str())
            .map_err(|e| Error::report(eyre::eyre!("{e}")))?;

        let grpc_channel = tonic::transport::Channel::from_shared(config.grpc_addr.clone())
            .map_err(Error::report)?
            .connect()
            .await
            .map_err(Error::report)?;

        let key_bytes = std::fs::read_to_string(&config.key_file).map_err(Error::report)?;
        let key_hex = key_bytes.trim();
        let secret_key_bytes = hex::decode(key_hex).map_err(Error::report)?;
        let secret_key_array: [u8; 32] = secret_key_bytes
            .try_into()
            .map_err(|_| Error::report(eyre::eyre!("secret key must be 32 bytes")))?;
        let secret_key =
            secp256k1::SecretKey::from_byte_array(secret_key_array).map_err(Error::report)?;
        let signer = Secp256k1KeyPair::from_secret_key(secret_key, &config.account_prefix);

        Ok(Self {
            block_time: config.block_time,
            config,
            chain_id,
            rpc_client,
            grpc_channel,
            signer,
            nonce_mutex: Arc::new(Mutex::new(None)),
        })
    }

    #[must_use]
    pub const fn chain_id(&self) -> &ChainId {
        &self.chain_id
    }

    #[must_use]
    pub const fn block_time(&self) -> Duration {
        self.block_time
    }
}

impl HasChainTypes for CosmosChain {
    type Height = TmHeight;
    type Timestamp = TmTime;
    type ChainId = ChainId;
    type Event = CosmosEvent;
}

impl HasMessageTypes for CosmosChain {
    type Message = CosmosMessage;
    type MessageResponse = CosmosTxResponse;
}

impl HasIbcTypes<Self> for CosmosChain {
    type ClientId = ibc::core::host::types::identifiers::ClientId;
    type ClientState = TendermintClientState;
    type ConsensusState = TendermintConsensusState;
    type CommitmentProof = MerkleProof;
}

impl HasPacketTypes<Self> for CosmosChain {
    type Packet = CosmosPacket;
    type PacketCommitment = PacketCommitment;
    type PacketReceipt = PacketReceipt;
    type Acknowledgement = PacketAcknowledgement;
}

impl HasChainStatusType for CosmosChain {
    type ChainStatus = CosmosChainStatus;

    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height {
        &status.height
    }

    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp {
        &status.timestamp
    }
}
