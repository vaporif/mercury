use std::path::PathBuf;

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
        let registry = crate::registry::build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        let config_dir = self
            .config
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));

        let host_cfg = cfg.find_chain(&registry, &self.host_chain)?;
        let _ref_cfg = cfg.find_chain(&registry, &self.reference_chain)?;

        let plugin = registry.chain(&host_cfg.chain_type)?;
        let _chain = plugin.connect(&host_cfg.raw, config_dir).await?;

        todo!("implement create client on chain '{}'", self.host_chain)
    }
}
