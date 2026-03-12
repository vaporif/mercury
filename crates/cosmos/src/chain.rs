use std::sync::Arc;
use std::time::Duration;

use ibc::core::host::types::identifiers::ChainId;
use ibc_client_tendermint::types::ClientState as TendermintClientState;
use ibc_client_tendermint::types::ConsensusState as TendermintConsensusState;
use tendermint::Time as TmTime;
use tendermint::block::Height as TmHeight;
use tendermint_rpc::HttpClient;
use tokio::sync::Mutex;

use mercury_chain_traits::types::{
    HasChainStatusType, HasChainTypes, HasIbcTypes, HasMessageTypes, HasPacketTypes,
};
use mercury_core::runtime::TokioRuntime;

use crate::config::CosmosChainConfig;
use crate::types::{
    CosmosChainStatus, CosmosEvent, CosmosMessage, CosmosPacket, CosmosTxResponse, MerkleProof,
    PacketAcknowledgement, PacketCommitment, PacketReceipt,
};

#[derive(Clone)]
pub struct CosmosChain {
    pub config: CosmosChainConfig,
    pub chain_id: ChainId,
    pub runtime: TokioRuntime,
    pub rpc_client: HttpClient,
    pub block_time: Duration,
    pub nonce_mutex: Arc<Mutex<()>>,
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

impl CosmosChain {
    #[must_use]
    pub const fn chain_id(&self) -> &ChainId {
        &self.chain_id
    }

    #[must_use]
    pub const fn block_time(&self) -> Duration {
        self.block_time
    }
}
