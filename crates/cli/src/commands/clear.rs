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
        todo!()
    }
}
