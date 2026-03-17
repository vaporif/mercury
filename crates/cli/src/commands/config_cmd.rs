use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::registry::build_registry;

#[derive(Subcommand)]
pub enum ConfigCmd {
    /// Validate the relayer configuration
    Validate(ConfigValidateCmd),
}

impl ConfigCmd {
    pub async fn run(self) -> eyre::Result<()> {
        match self {
            Self::Validate(cmd) => cmd.run().await,
        }
    }
}

#[derive(Args)]
pub struct ConfigValidateCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
}

impl ConfigValidateCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        println!("Configuration is valid.");
        println!("  Chains: {}", cfg.chains.len());
        println!("  Relays: {}", cfg.relays.len());
        Ok(())
    }
}
