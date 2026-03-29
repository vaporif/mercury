use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum KeysCmd {
    /// Add a key for a chain
    Add(KeysAddCmd),
    /// Delete a key
    Delete(KeysDeleteCmd),
    /// List keys for a chain
    List(KeysListCmd),
    /// Query key balance
    Balance(KeysBalanceCmd),
}

impl KeysCmd {
    pub async fn run(self) -> eyre::Result<()> {
        match self {
            Self::Add(cmd) => cmd.run().await,
            Self::Delete(cmd) => cmd.run().await,
            Self::List(cmd) => cmd.run().await,
            Self::Balance(cmd) => cmd.run().await,
        }
    }
}

#[derive(Args)]
pub struct KeysAddCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID
    #[arg(long)]
    chain: String,
    /// Path to raw key file
    #[arg(long)]
    key_file: PathBuf,
    /// Optional alias for the key
    #[arg(long)]
    key_name: Option<String>,
}

impl KeysAddCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let _cfg = crate::config::load_config(&self.config, &registry)?;

        todo!("implement keys add for chain '{}'", self.chain)
    }
}

#[derive(Args)]
pub struct KeysDeleteCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID
    #[arg(long)]
    chain: String,
    /// Key name to delete
    #[arg(long)]
    key_name: String,
}

impl KeysDeleteCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let _cfg = crate::config::load_config(&self.config, &registry)?;

        todo!("implement keys delete for chain '{}'", self.chain)
    }
}

#[derive(Args)]
pub struct KeysListCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID
    #[arg(long)]
    chain: String,
}

impl KeysListCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let _cfg = crate::config::load_config(&self.config, &registry)?;

        todo!("implement keys list for chain '{}'", self.chain)
    }
}

#[derive(Args)]
pub struct KeysBalanceCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID
    #[arg(long)]
    chain: String,
    /// Token denomination (defaults to gas denom from config)
    #[arg(long)]
    denom: Option<String>,
}

impl KeysBalanceCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let _cfg = crate::config::load_config(&self.config, &registry)?;

        todo!("implement keys balance for chain '{}'", self.chain)
    }
}
