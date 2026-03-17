use std::path::{Path, PathBuf};

use clap::Args;

use crate::registry::build_registry;

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
    let registry = build_registry();
    let cfg = crate::config::load_config(config_path, &registry)?;
    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));

    let chain_cfg = cfg.find_chain(&registry, chain_id)?;

    let plugin = registry.chain(&chain_cfg.chain_type)?;
    let rpc_addr = plugin.rpc_addr_from_config(&chain_cfg.raw)?;
    let chain = plugin.connect(&chain_cfg.raw, config_dir).await?;

    println!("Chain:     {chain_id}");
    println!("RPC:       {rpc_addr}");

    match plugin.query_status(&chain).await {
        Ok(info) => {
            println!("Height:    {}", info.height);
            println!("Timestamp: {}", info.timestamp);
            println!("Status:    reachable");
        }
        Err(e) => {
            println!("Status:    unreachable ({e})");
        }
    }

    Ok(())
}
