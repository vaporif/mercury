use std::path::PathBuf;

use clap::Args;

#[derive(Args)]
pub struct MisbehaviourCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID
    #[arg(long)]
    chain: String,
    /// Client ID
    #[arg(long)]
    client: String,
    /// Number of past blocks to check (default: 100)
    #[arg(long, default_value = "100")]
    check_past_blocks: u64,
}

impl MisbehaviourCmd {
    pub async fn run(self) -> eyre::Result<()> {
        todo!()
    }
}
