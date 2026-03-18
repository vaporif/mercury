use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum CreateCmd {
    Client(CreateClientCmd),
}

impl CreateCmd {
    pub async fn run(self) -> eyre::Result<()> {
        match self {
            Self::Client(cmd) => cmd.run().await,
        }
    }
}

#[derive(Args)]
pub struct CreateClientCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain where the client will be created
    #[arg(long)]
    host_chain: String,
    /// Chain being tracked by the client
    #[arg(long)]
    reference_chain: String,
}

impl CreateClientCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        let config_dir = self.config.parent().unwrap_or_else(|| Path::new("."));

        let host_cfg = cfg.find_chain(&registry, &self.host_chain)?;
        let ref_cfg = cfg.find_chain(&registry, &self.reference_chain)?;

        let host_plugin = registry.chain(&host_cfg.chain_type)?;
        let ref_plugin = registry.chain(&ref_cfg.chain_type)?;

        host_plugin.validate_config(&host_cfg.raw)?;
        ref_plugin.validate_config(&ref_cfg.raw)?;

        let host_chain = host_plugin.connect(&host_cfg.raw, config_dir).await?;
        let ref_chain = ref_plugin.connect(&ref_cfg.raw, config_dir).await?;

        let payload = ref_plugin.build_create_client_payload(&ref_chain).await?;
        let client_id = host_plugin.create_client(&host_chain, payload).await?;

        println!("Created client {client_id} on {}", self.host_chain);
        Ok(())
    }
}
