use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum CreateCmd {
    /// Create a light client
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
    /// Client ID on the reference chain (for `MsgRegisterCounterparty`)
    #[arg(long)]
    counterparty_client: String,
    /// Trusting period (e.g. "14days")
    #[arg(long)]
    trusting_period: Option<String>,
    /// Trust threshold (e.g. "2/3")
    #[arg(long)]
    trust_threshold: Option<String>,
    /// Maximum clock drift
    #[arg(long)]
    max_clock_drift: Option<String>,
}

impl CreateClientCmd {
    pub async fn run(self) -> eyre::Result<()> {
        todo!()
    }
}
