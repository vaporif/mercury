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
            "implement misbehaviour detection for chain '{}'",
            self.chain
        )
    }
}
