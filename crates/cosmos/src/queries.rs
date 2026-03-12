use async_trait::async_trait;
use tendermint_rpc::Client;

use mercury_chain_traits::queries::{
    CanQueryChainStatus, CanQueryClientState, CanQueryConsensusState,
};
use mercury_chain_traits::types::HasChainTypes;
use mercury_core::error::{Error, Result};

use crate::chain::CosmosChain;
use crate::types::CosmosChainStatus;

#[async_trait]
impl CanQueryChainStatus for CosmosChain {
    async fn query_chain_status(&self) -> Result<Self::ChainStatus> {
        let status = self.rpc_client.status().await.map_err(Error::report)?;
        Ok(CosmosChainStatus {
            height: status.sync_info.latest_block_height,
            timestamp: status.sync_info.latest_block_time,
        })
    }
}

#[async_trait]
impl CanQueryClientState<Self> for CosmosChain {
    async fn query_client_state(
        &self,
        _client_id: &Self::ClientId,
        _height: &Self::Height,
    ) -> Result<Self::ClientState> {
        // TODO: implement via gRPC query to ibc.core.client.v2.Query/ClientState
        todo!("query client state via gRPC")
    }
}

#[async_trait]
impl CanQueryConsensusState<Self> for CosmosChain {
    async fn query_consensus_state(
        &self,
        _client_id: &Self::ClientId,
        _consensus_height: &<Self as HasChainTypes>::Height,
        _query_height: &Self::Height,
    ) -> Result<Self::ConsensusState> {
        // TODO: implement via gRPC query to ibc.core.client.v2.Query/ConsensusState
        todo!("query consensus state via gRPC")
    }
}
