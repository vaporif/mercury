use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum ClearCmd {
    /// Clear pending packets
    Packets(ClearPacketsCmd),
}

impl ClearCmd {
    pub async fn run(self) -> eyre::Result<()> {
        match self {
            Self::Packets(cmd) => cmd.run().await,
        }
    }
}

#[derive(Args)]
pub struct ClearPacketsCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Source chain ID
    #[arg(long)]
    chain: String,
    /// Client ID on the source chain
    #[arg(long)]
    client: String,
    /// Counterparty chain ID
    #[arg(long)]
    counterparty_chain: String,
    /// Counterparty client ID
    #[arg(long)]
    counterparty_client: String,
    /// Specific sequences to clear (e.g. "1,5,10..20")
    #[arg(long)]
    sequences: Option<String>,
}

impl ClearPacketsCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        let config_dir = self
            .config
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));

        let chain_cfg = cfg.find_chain(&registry, &self.chain)?;
        let _counterparty_cfg = cfg.find_chain(&registry, &self.counterparty_chain)?;

        let plugin = registry.chain(&chain_cfg.chain_type)?;
        let _chain = plugin.connect(&chain_cfg.raw, config_dir).await?;

        todo!("implement clear packets for chain '{}'", self.chain)
    }
}
