use std::path::{Path, PathBuf};

use clap::Args;

use crate::registry::build_registry;

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
        let registry = build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        let config_dir = self.config.parent().unwrap_or_else(|| Path::new("."));

        let chains_to_check: Vec<_> = if let Some(ref target) = self.chain {
            vec![cfg.find_chain(&registry, target)?]
        } else {
            cfg.chains.iter().collect()
        };

        if chains_to_check.is_empty() {
            println!("No chains configured.");
            return Ok(());
        }

        let mut all_healthy = true;
        for chain_cfg in &chains_to_check {
            let plugin = registry.chain(&chain_cfg.chain_type)?;
            let chain_id = plugin.chain_id_from_config(&chain_cfg.raw)?;

            let result = async {
                let chain = plugin.connect(&chain_cfg.raw, config_dir).await?;
                plugin.query_status(&chain).await
            }
            .await;

            match result {
                Ok(info) => println!(
                    "{chain_id}: healthy (height={}, ts={})",
                    info.height, info.timestamp
                ),
                Err(e) => {
                    println!("{chain_id}: unhealthy ({e})");
                    all_healthy = false;
                }
            }
        }

        if !all_healthy {
            eyre::bail!("one or more chains are unhealthy");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{HUB, err_string, example_config, write_config};

    fn cmd(config: &Path, chain: Option<&str>) -> HealthCheckCmd {
        HealthCheckCmd {
            config: config.into(),
            chain: chain.map(Into::into),
        }
    }

    #[tokio::test]
    async fn config_without_chains_is_healthy() {
        let file = write_config("");
        cmd(&file.path, None).run().await.unwrap();
    }

    #[tokio::test]
    async fn unknown_target_chain_is_reported() {
        let cfg = example_config();
        let err = err_string(cmd(&cfg.path, Some("juno-1")).run().await);
        assert_eq!(err, "chain 'juno-1' not found in config");
    }

    #[tokio::test]
    async fn unreachable_chains_make_check_fail() {
        let cfg = example_config();
        for target in [None, Some(HUB)] {
            let err = err_string(cmd(&cfg.path, target).run().await);
            assert_eq!(err, "one or more chains are unhealthy", "{target:?}");
        }
    }
}
