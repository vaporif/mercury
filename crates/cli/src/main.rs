use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use mercury_cosmos::chain::CosmosChain;
use mercury_cosmos::keys::{Secp256k1KeyPair, load_cosmos_signer};
use mercury_relay::context::RelayContext;
use tokio::task::JoinHandle;
use tracing_subscriber::EnvFilter;

mod config;

use config::{ChainConfig, RelayConfig};

#[derive(Parser)]
#[command(name = "mercury", about = "IBC v2 relayer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the relayer
    Start {
        /// Path to config file
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Query chain status
    Status {
        /// Path to config file
        #[arg(short, long)]
        config: PathBuf,
        /// Chain ID to query
        #[arg(long)]
        chain: String,
    },
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start { config } => {
            run_start(&config).await?;
        }
        Commands::Status { config, chain } => {
            run_status(&config, &chain).await?;
        }
    }

    Ok(())
}

async fn run_status(config_path: &Path, chain_id: &str) -> eyre::Result<()> {
    let cfg = config::load_config(config_path)?;

    let chain_config = cfg
        .chains
        .iter()
        .find(|c| c.chain_id() == chain_id)
        .ok_or_else(|| eyre::eyre!("chain '{chain_id}' not found in config"))?;

    let rpc_addr = chain_config.rpc_addr();

    println!("Chain:     {chain_id}");
    println!("RPC:       {rpc_addr}");

    match mercury_cosmos::status::query_cosmos_status(rpc_addr).await {
        Ok(status) => {
            println!("Height:    {}", status.height);
            println!("Timestamp: {}", status.timestamp);
            println!("Status:    reachable");
        }
        Err(e) => {
            println!("Status:    unreachable ({e})");
        }
    }

    Ok(())
}

#[derive(Clone)]
enum ConnectedChain {
    Cosmos(CosmosChain<Secp256k1KeyPair>),
}

async fn run_start(config_path: &Path) -> eyre::Result<()> {
    let cfg = config::load_config(config_path)?;
    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));

    for relay in &cfg.relays {
        if !cfg.chains.iter().any(|c| c.chain_id() == relay.src_chain) {
            eyre::bail!("relay references unknown src_chain '{}'", relay.src_chain);
        }
        if !cfg.chains.iter().any(|c| c.chain_id() == relay.dst_chain) {
            eyre::bail!("relay references unknown dst_chain '{}'", relay.dst_chain);
        }
    }

    let mut chains: HashMap<String, ConnectedChain> = HashMap::new();
    for chain_config in &cfg.chains {
        let chain = connect_chain(chain_config, config_dir).await?;
        let id = chain_config.chain_id().to_string();
        tracing::info!(chain_id = %id, "connected to chain");
        chains.insert(id, chain);
    }

    let mut handles: Vec<JoinHandle<mercury_core::error::Result<()>>> = Vec::new();
    for relay in &cfg.relays {
        let src = chains
            .get(&relay.src_chain)
            .ok_or_else(|| eyre::eyre!("chain '{}' not in cache", relay.src_chain))?
            .clone();
        let dst = chains
            .get(&relay.dst_chain)
            .ok_or_else(|| eyre::eyre!("chain '{}' not in cache", relay.dst_chain))?
            .clone();

        let handle = spawn_relay_pair(src, dst, relay)?;
        tracing::info!(
            src = %relay.src_chain,
            dst = %relay.dst_chain,
            "spawned bidirectional relay"
        );
        handles.push(handle);
    }

    if handles.is_empty() {
        tracing::warn!("no relay pairs configured — nothing to do");
        return Ok(());
    }

    tracing::info!(count = handles.len(), "all relay pairs running");

    tokio::select! {
        (result, _index, remaining) = futures::future::select_all(handles) => {
            match result {
                Ok(Ok(())) => tracing::warn!("relay pair exited unexpectedly"),
                Ok(Err(e)) => tracing::error!(error = %e, "relay pair failed"),
                Err(e) => tracing::error!(error = %e, "relay task panicked"),
            }
            for handle in remaining {
                handle.abort();
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received ctrl-c, shutting down");
        }
    }

    Ok(())
}

async fn connect_chain(
    chain_config: &ChainConfig,
    config_dir: &Path,
) -> eyre::Result<ConnectedChain> {
    match chain_config {
        ChainConfig::Cosmos(cosmos_cfg) => {
            let key_path = config_dir.join(&cosmos_cfg.key_file);

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&key_path) {
                    let mode = meta.permissions().mode();
                    if mode & 0o044 != 0 {
                        tracing::warn!(
                            path = %key_path.display(),
                            "key file is readable by group/others — consider chmod 600"
                        );
                    }
                }
            }

            let signer = load_cosmos_signer(&key_path, &cosmos_cfg.account_prefix)
                .map_err(|e| eyre::eyre!("loading signer for '{}': {e}", cosmos_cfg.chain_id))?;

            let chain = CosmosChain::new(cosmos_cfg.clone(), signer)
                .await
                .map_err(|e| eyre::eyre!("connecting to '{}': {e}", cosmos_cfg.chain_id))?;

            let on_chain_id = chain.chain_id.to_string();
            if on_chain_id != cosmos_cfg.chain_id {
                eyre::bail!(
                    "chain_id mismatch for '{}': config says '{}', node reports '{}'",
                    cosmos_cfg.chain_id,
                    cosmos_cfg.chain_id,
                    on_chain_id,
                );
            }

            Ok(ConnectedChain::Cosmos(chain))
        }
    }
}

fn spawn_relay_pair(
    src: ConnectedChain,
    dst: ConnectedChain,
    relay: &RelayConfig,
) -> eyre::Result<JoinHandle<mercury_core::error::Result<()>>> {
    match (src, dst) {
        (ConnectedChain::Cosmos(src_chain), ConnectedChain::Cosmos(dst_chain)) => {
            let src_client_id: ibc::core::host::types::identifiers::ClientId = relay
                .src_client_id
                .parse()
                .map_err(|e| eyre::eyre!("invalid src_client_id '{}': {e}", relay.src_client_id))?;
            let dst_client_id: ibc::core::host::types::identifiers::ClientId = relay
                .dst_client_id
                .parse()
                .map_err(|e| eyre::eyre!("invalid dst_client_id '{}': {e}", relay.dst_client_id))?;

            let fwd = Arc::new(RelayContext {
                src_chain: src_chain.clone(),
                dst_chain: dst_chain.clone(),
                src_client_id: src_client_id.clone(),
                dst_client_id: dst_client_id.clone(),
            });

            let rev = Arc::new(RelayContext {
                src_chain: dst_chain,
                dst_chain: src_chain,
                src_client_id: dst_client_id,
                dst_client_id: src_client_id,
            });

            let src_name = relay.src_chain.clone();
            let dst_name = relay.dst_chain.clone();

            Ok(tokio::spawn(async move {
                tracing::info!(
                    src = %src_name,
                    dst = %dst_name,
                    "running bidirectional relay"
                );
                let (res_a, res_b) = tokio::join!(Arc::clone(&fwd).run(), Arc::clone(&rev).run(),);
                if let Err(ref e) = res_a {
                    tracing::error!(direction = "a→b", error = %e, "relay direction failed");
                }
                if let Err(ref e) = res_b {
                    tracing::error!(direction = "b→a", error = %e, "relay direction failed");
                }
                res_a.and(res_b)
            }))
        }
    }
}
