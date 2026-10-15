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

        eyre::bail!(
            "misbehaviour detection for chain '{}' is not implemented yet",
            self.chain
        )
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::test_support::{HUB, err_string, example_config};

    #[derive(Parser)]
    struct Wrap {
        #[command(flatten)]
        cmd: MisbehaviourCmd,
    }

    #[test]
    fn check_past_blocks_defaults_to_documented_100() {
        let w =
            Wrap::try_parse_from(["x", "-c", "r.toml", "--chain", "a", "--client", "b"]).unwrap();
        assert_eq!(w.cmd.check_past_blocks, 100);
    }

    fn cmd(config: &std::path::Path, chain: &str) -> MisbehaviourCmd {
        MisbehaviourCmd {
            config: config.into(),
            chain: chain.into(),
            client: "07-tendermint-0".into(),
            check_past_blocks: 100,
        }
    }

    #[tokio::test]
    async fn unknown_chain_is_reported() {
        let cfg = example_config();
        let err = err_string(cmd(&cfg.path, "juno-1").run().await);
        assert_eq!(err, "chain 'juno-1' not found in config");
    }

    #[tokio::test]
    async fn connects_before_reporting_not_implemented() {
        // Pins current behaviour: a chain that cannot connect never reaches
        // the "not implemented" message.
        let cfg = example_config();
        let err = err_string(cmd(&cfg.path, HUB).run().await);
        assert!(
            err.starts_with(&format!("loading signer for '{HUB}'")),
            "{err}"
        );
    }
}
