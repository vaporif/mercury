use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use tracing::info;

#[derive(Subcommand)]
pub enum UpdateCmd {
    /// Update a light client
    Client(UpdateClientCmd),
}

impl UpdateCmd {
    pub async fn run(self) -> eyre::Result<()> {
        match self {
            Self::Client(cmd) => cmd.run().await,
        }
    }
}

#[derive(Args)]
pub struct UpdateClientCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain where the client lives
    #[arg(long)]
    host_chain: String,
    /// Client ID to update
    #[arg(long)]
    client: String,
    /// Chain the client tracks (source of headers); auto-detected if omitted
    #[arg(long)]
    reference_chain: Option<String>,
    /// Target height to update to; defaults to latest
    #[arg(long)]
    height: Option<u64>,
}

impl UpdateClientCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        let config_dir = self.config.parent().unwrap_or_else(|| Path::new("."));

        let host_cfg = cfg.find_chain(&registry, &self.host_chain)?;
        let host_plugin = registry.chain(&host_cfg.chain_type)?;
        host_plugin.validate_config(&host_cfg.raw)?;
        let host_chain = host_plugin.connect(&host_cfg.raw, config_dir).await?;

        let client_info = host_plugin
            .query_client_state_info(&host_chain, &self.client, None)
            .await?;
        let trusted_height = client_info.latest_height;

        if client_info.frozen {
            eyre::bail!("client '{}' is frozen, cannot update", self.client);
        }

        let ref_chain_id = match &self.reference_chain {
            Some(id) => id.clone(),
            None => {
                if client_info.chain_id.is_empty() {
                    eyre::bail!(
                        "cannot auto-detect reference chain for wasm client; use --reference-chain"
                    );
                }
                client_info.chain_id.clone()
            }
        };

        let explicit = self.reference_chain.is_some();
        let ref_cfg = cfg.find_chain(&registry, &ref_chain_id).map_err(|e| {
            if explicit {
                eyre::eyre!("could not find chain with id '{ref_chain_id}' in config: {e}")
            } else {
                eyre::eyre!("could not find chain with id '{ref_chain_id}' in config; use --reference-chain to specify explicitly")
            }
        })?;
        let ref_plugin = registry.chain(&ref_cfg.chain_type)?;
        ref_plugin.validate_config(&ref_cfg.raw)?;
        let ref_chain = ref_plugin.connect(&ref_cfg.raw, config_dir).await?;

        let target_height = match self.height {
            Some(h) => h,
            None => {
                let status = ref_plugin.query_status(&ref_chain).await?;
                status.height
            }
        };

        if target_height <= trusted_height {
            info!(
                client_id = %self.client,
                trusted_height,
                target_height,
                "client already up to date"
            );
            return Ok(());
        }

        let payload = ref_plugin
            .build_update_client_payload(&ref_chain, trusted_height, target_height, None)
            .await?;

        host_plugin
            .update_client(&host_chain, &self.client, payload)
            .await?;

        info!(
            client_id = %self.client,
            host = %self.host_chain,
            reference = %ref_chain_id,
            trusted_height,
            target_height,
            "client updated"
        );
        Ok(())
    }
}
