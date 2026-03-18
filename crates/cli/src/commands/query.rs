use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum QueryCmd {
    /// Query client state
    #[command(subcommand)]
    Client(QueryClientCmd),
    /// Query packet information
    #[command(subcommand)]
    Packet(QueryPacketCmd),
}

impl QueryCmd {
    pub async fn run(self) -> eyre::Result<()> {
        match self {
            Self::Client(cmd) => cmd.run().await,
            Self::Packet(cmd) => cmd.run().await,
        }
    }
}

#[derive(Subcommand)]
pub enum QueryClientCmd {
    /// Query client state
    State(QueryClientStateCmd),
}

impl QueryClientCmd {
    pub async fn run(self) -> eyre::Result<()> {
        match self {
            Self::State(cmd) => cmd.run().await,
        }
    }
}

#[derive(Args)]
pub struct QueryClientStateCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID
    #[arg(long)]
    chain: String,
    /// Client ID
    #[arg(long)]
    client: String,
    /// Query at specific height
    #[arg(long)]
    height: Option<u64>,
}

impl QueryClientStateCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        let config_dir = self
            .config
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));

        let chain_cfg = cfg.find_chain(&registry, &self.chain)?;

        let _chain = registry
            .chain(&chain_cfg.chain_type)?
            .connect(&chain_cfg.raw, config_dir)
            .await?;

        todo!("implement query client state for chain '{}'", self.chain)
    }
}

#[derive(Subcommand)]
pub enum QueryPacketCmd {
    /// Query packet commitments
    Commitments(QueryPacketCommitmentsCmd),
    /// Query pending packets
    Pending(QueryPacketPendingCmd),
}

impl QueryPacketCmd {
    pub async fn run(self) -> eyre::Result<()> {
        match self {
            Self::Commitments(cmd) => cmd.run().await,
            Self::Pending(cmd) => cmd.run().await,
        }
    }
}

#[derive(Args)]
pub struct QueryPacketCommitmentsCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID
    #[arg(long)]
    chain: String,
    /// Client ID
    #[arg(long)]
    client: String,
}

impl QueryPacketCommitmentsCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        let config_dir = self
            .config
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));

        let chain_cfg = cfg.find_chain(&registry, &self.chain)?;

        let _chain = registry
            .chain(&chain_cfg.chain_type)?
            .connect(&chain_cfg.raw, config_dir)
            .await?;

        todo!(
            "implement query packet commitments for chain '{}'",
            self.chain
        )
    }
}

#[derive(Args)]
pub struct QueryPacketPendingCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Source chain ID
    #[arg(long)]
    chain: String,
    /// Source client ID
    #[arg(long)]
    client: String,
    /// Counterparty chain ID (to check receipts)
    #[arg(long)]
    counterparty_chain: String,
    /// Counterparty client ID
    #[arg(long)]
    counterparty_client: String,
}

impl QueryPacketPendingCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        let config_dir = self
            .config
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));

        let chain_cfg = cfg.find_chain(&registry, &self.chain)?;

        let _counterparty_cfg = cfg.find_chain(&registry, &self.counterparty_chain)?;

        let _chain = registry
            .chain(&chain_cfg.chain_type)?
            .connect(&chain_cfg.raw, config_dir)
            .await?;

        todo!("implement query packet pending for chain '{}'", self.chain)
    }
}
