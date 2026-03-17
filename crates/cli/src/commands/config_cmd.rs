use std::path::PathBuf;

use clap::{Args, Subcommand};

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
        todo!()
    }
}
