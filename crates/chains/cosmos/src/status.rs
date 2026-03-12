use mercury_core::error::{Error, Result};
use tendermint_rpc::{Client, HttpClient};

use crate::types::CosmosChainStatus;

/// Query chain status via RPC only. No keys, no gRPC — lightweight health check.
pub async fn query_cosmos_status(rpc_addr: &str) -> Result<CosmosChainStatus> {
    let client = HttpClient::new(rpc_addr).map_err(Error::report)?;
    let status = client.status().await.map_err(Error::report)?;
    Ok(CosmosChainStatus {
        height: status.sync_info.latest_block_height,
        timestamp: status.sync_info.latest_block_time,
    })
}
