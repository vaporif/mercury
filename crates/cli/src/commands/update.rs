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
        todo!()
    }
}
