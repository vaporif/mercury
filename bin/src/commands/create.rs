use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use tracing::info;

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
}

impl CreateClientCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        let config_dir = self.config.parent().unwrap_or_else(|| Path::new("."));

        let host_cfg = cfg.find_chain(&registry, &self.host_chain)?;
        let ref_cfg = cfg.find_chain(&registry, &self.reference_chain)?;

        let host_plugin = registry.chain(&host_cfg.chain_type)?;
        let ref_plugin = registry.chain(&ref_cfg.chain_type)?;

        host_plugin.validate_config(&host_cfg.raw)?;
        ref_plugin.validate_config(&ref_cfg.raw)?;

        let host_chain = host_plugin.connect(&host_cfg.raw, config_dir).await?;
        let ref_chain = ref_plugin.connect(&ref_cfg.raw, config_dir).await?;

        let payload = ref_plugin.build_create_client_payload(&ref_chain).await?;
        let client_id = host_plugin.create_client(&host_chain, payload).await?;

        info!(
            %client_id,
            host = %self.host_chain,
            reference = %self.reference_chain,
            "client created"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{HUB, OSMO, err_string, example_config};

    fn cmd(config: &Path, host: &str, reference: &str) -> CreateCmd {
        CreateCmd::Client(CreateClientCmd {
            config: config.into(),
            host_chain: host.into(),
            reference_chain: reference.into(),
        })
    }

    #[tokio::test]
    async fn unknown_host_or_reference_chain_is_reported() {
        let cfg = example_config();
        for (host, reference) in [("juno-1", OSMO), (HUB, "juno-1")] {
            let err = err_string(cmd(&cfg.path, host, reference).run().await);
            assert_eq!(err, "chain 'juno-1' not found in config");
        }
    }

    #[tokio::test]
    async fn host_connect_failure_aborts_creation() {
        let cfg = example_config();
        let err = err_string(cmd(&cfg.path, HUB, OSMO).run().await);
        assert!(
            err.starts_with(&format!("loading signer for '{HUB}'")),
            "{err}"
        );
    }
}
