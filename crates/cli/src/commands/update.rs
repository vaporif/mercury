use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum UpdateCmd {
    /// Update a light client
    Client(UpdateClientCmd),
}

impl UpdateCmd {
    pub async fn run(self) -> eyre::Result<()> {
        match self {
            Self::Client(cmd) => cmd.run().await,
        }
    }
}

#[derive(Args)]
pub struct UpdateClientCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain where the client lives
    #[arg(long)]
    host_chain: String,
    /// Client ID to update
    #[arg(long)]
    client: String,
    /// Target height
    #[arg(long)]
    height: Option<u64>,
}

impl UpdateClientCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        let config_dir = self
            .config
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));

        let chain_cfg = cfg.find_chain(&registry, &self.host_chain)?;

        let plugin = registry.chain(&chain_cfg.chain_type)?;
        let _chain = plugin.connect(&chain_cfg.raw, config_dir).await?;

        todo!("implement update client on chain '{}'", self.host_chain)
    }
}
