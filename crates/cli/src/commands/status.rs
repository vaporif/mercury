use std::path::{Path, PathBuf};

use clap::Args;

use crate::config::ChainConfig;

#[derive(Args)]
pub struct StatusCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID to query
    #[arg(long)]
    chain: String,
}

impl StatusCmd {
    pub async fn run(self) -> eyre::Result<()> {
        run_status(&self.config, &self.chain).await
    }
}

async fn run_status(config_path: &Path, chain_id: &str) -> eyre::Result<()> {
    let cfg = crate::config::load_config(config_path)?;

    let chain_config = cfg
        .chains
        .iter()
        .find(|c| c.chain_id() == chain_id)
        .ok_or_else(|| eyre::eyre!("chain '{chain_id}' not found in config"))?;

    let rpc_addr = chain_config.rpc_addr();

    println!("Chain:     {chain_id}");
    println!("RPC:       {rpc_addr}");

    match chain_config {
        ChainConfig::Cosmos(_) => {
            match mercury_cosmos_counterparties::queries::query_cosmos_status(rpc_addr).await {
                Ok(status) => {
                    println!("Height:    {}", status.height);
                    println!("Timestamp: {}", status.timestamp);
                    println!("Status:    reachable");
                }
                Err(e) => {
                    println!("Status:    unreachable ({e})");
                }
            }
        }
        ChainConfig::Ethereum(cfg) => {
            use alloy::eips::BlockNumberOrTag;
            use alloy::providers::{Provider, ProviderBuilder};

            let url: url::Url = cfg
                .rpc_addr
                .parse()
                .map_err(|e| eyre::eyre!("invalid Ethereum RPC URL: {e}"))?;
            let provider = ProviderBuilder::new().connect_http(url);

            match provider.get_block_by_number(BlockNumberOrTag::Latest).await {
                Ok(Some(block)) => {
                    println!("Height:    {}", block.header.number);
                    println!("Timestamp: {}", block.header.timestamp);
                    println!("Status:    reachable");
                }
                Ok(None) => {
                    println!("Status:    unreachable (no block returned)");
                }
                Err(e) => {
                    println!("Status:    unreachable ({e})");
                }
            }
        }
    }

    Ok(())
}
