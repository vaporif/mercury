use mercury_core::error::{Error, Result};
use tendermint::block::Height as TmHeight;
use tendermint_rpc::{Client, HttpClient};

pub async fn query_latest_height(client: &HttpClient) -> Result<TmHeight> {
    let status = client.status().await.map_err(Error::report)?;
    Ok(status.sync_info.latest_block_height)
}

pub async fn query_abci(
    client: &HttpClient,
    path: &str,
    data: Vec<u8>,
    height: Option<TmHeight>,
    prove: bool,
) -> Result<tendermint_rpc::endpoint::abci_query::AbciQuery> {
    let response = client
        .abci_query(Some(path.to_string()), data, height, prove)
        .await
        .map_err(Error::report)?;
    Ok(response)
}
