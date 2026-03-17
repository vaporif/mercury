use std::path::PathBuf;

use clap::Args;

#[derive(Args)]
pub struct HealthCheckCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID to check (omit to check all chains)
    #[arg(long)]
    chain: Option<String>,
}

impl HealthCheckCmd {
    pub async fn run(self) -> eyre::Result<()> {
        todo!()
    }
}
