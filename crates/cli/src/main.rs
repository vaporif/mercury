use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use futures::FutureExt;
use mercury_cosmos_bridges::CosmosChain;
use mercury_cosmos_bridges::keys::{Secp256k1KeyPair, load_cosmos_signer};
use mercury_ethereum::types::EvmClientId;
use mercury_ethereum_bridges::EthereumChain;
use mercury_relay::context::{RelayContext, RelayWorkerConfig};
use mercury_relay::filter::PacketFilter;
use tokio::task::JoinHandle;
use tracing::instrument;
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
        /// Optional health-check port — serves HTTP 200 once relays are running
        #[arg(long)]
        health_port: Option<u16>,
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
        Commands::Start {
            config,
            health_port,
        } => {
            run_start(&config, health_port).await?;
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

    match chain_config {
        ChainConfig::Cosmos(_) => {
            match mercury_cosmos_bridges::queries::query_cosmos_status(rpc_addr).await {
                Ok(status) => {
                    println!("Height:    {}", status.height);
                    println!("Timestamp: {}", status.timestamp);
                    println!("Status:    reachable");
                }
                Err(e) => {
                    println!("Status:    unreachable ({e})");
                }
            }
        }
        ChainConfig::Ethereum(cfg) => {
            use alloy::eips::BlockNumberOrTag;
            use alloy::providers::{Provider, ProviderBuilder};

            let url: url::Url = cfg
                .rpc_addr
                .parse()
                .map_err(|e| eyre::eyre!("invalid Ethereum RPC URL: {e}"))?;
            let provider = ProviderBuilder::new().connect_http(url);

            match provider.get_block_by_number(BlockNumberOrTag::Latest).await {
                Ok(Some(block)) => {
                    println!("Height:    {}", block.header.number);
                    println!("Timestamp: {}", block.header.timestamp);
                    println!("Status:    reachable");
                }
                Ok(None) => {
                    println!("Status:    unreachable (no block returned)");
                }
                Err(e) => {
                    println!("Status:    unreachable ({e})");
                }
            }
        }
    }

    Ok(())
}

#[derive(Clone)]
enum ConnectedChain {
    Cosmos(Box<CosmosChain<Secp256k1KeyPair>>),
    Ethereum(Box<EthereumChain>),
}

trait DynRelay: Send + Sync {
    fn run(
        self: Arc<Self>,
        token: tokio_util::sync::CancellationToken,
        config: RelayWorkerConfig,
    ) -> futures::future::BoxFuture<'static, mercury_core::error::Result<()>>;
}

impl DynRelay for RelayContext<CosmosChain<Secp256k1KeyPair>, CosmosChain<Secp256k1KeyPair>> {
    fn run(
        self: Arc<Self>,
        token: tokio_util::sync::CancellationToken,
        config: RelayWorkerConfig,
    ) -> futures::future::BoxFuture<'static, mercury_core::error::Result<()>> {
        self.run_with_token(token, config).boxed()
    }
}

impl DynRelay for RelayContext<CosmosChain<Secp256k1KeyPair>, EthereumChain> {
    fn run(
        self: Arc<Self>,
        token: tokio_util::sync::CancellationToken,
        config: RelayWorkerConfig,
    ) -> futures::future::BoxFuture<'static, mercury_core::error::Result<()>> {
        self.run_with_token(token, config).boxed()
    }
}

impl DynRelay for RelayContext<EthereumChain, CosmosChain<Secp256k1KeyPair>> {
    fn run(
        self: Arc<Self>,
        token: tokio_util::sync::CancellationToken,
        config: RelayWorkerConfig,
    ) -> futures::future::BoxFuture<'static, mercury_core::error::Result<()>> {
        self.run_with_token(token, config).boxed()
    }
}

#[instrument(skip_all, name = "run_start")]
async fn run_start(config_path: &Path, health_port: Option<u16>) -> eyre::Result<()> {
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
        let id = chain_config.chain_id();
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

        let (fwd, rev) = build_relay_pair(src, dst, relay)?;
        let handle = spawn_relay_pair(fwd, rev, relay)?;
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

    if let Some(port) = health_port {
        tokio::spawn(serve_health(port));
    }

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

#[instrument(skip_all, name = "connect_chain")]
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

            let chain = CosmosChain::new(cosmos_cfg.as_ref().clone(), signer)
                .await
                .map_err(|e| eyre::eyre!("connecting to '{}': {e}", cosmos_cfg.chain_id))?;

            let on_chain_id = chain.chain_id.to_string();
            if on_chain_id != cosmos_cfg.chain_id {
                eyre::bail!(
                    "chain_id mismatch: config says '{}', node reports '{on_chain_id}'",
                    cosmos_cfg.chain_id,
                );
            }

            Ok(ConnectedChain::Cosmos(Box::new(chain)))
        }
        ChainConfig::Ethereum(eth_cfg) => {
            let key_path = config_dir.join(&eth_cfg.key_file);

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

            let signer = mercury_ethereum_bridges::keys::load_ethereum_signer(&key_path)
                .map_err(|e| eyre::eyre!("loading signer for chain {}: {e}", eth_cfg.chain_id))?;

            let chain = EthereumChain::new(eth_cfg.clone(), signer)
                .await
                .map_err(|e| eyre::eyre!("connecting to chain {}: {e}", eth_cfg.chain_id))?;

            Ok(ConnectedChain::Ethereum(Box::new(chain)))
        }
    }
}

async fn serve_health(port: u16) {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let addr = format!("127.0.0.1:{port}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => {
            tracing::info!(port, "health endpoint listening");
            l
        }
        Err(e) => {
            tracing::error!(port, error = %e, "failed to bind health endpoint");
            return;
        }
    };

    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    loop {
        if let Ok((mut stream, _)) = listener.accept().await {
            if let Err(e) = stream.write_all(response).await {
                tracing::debug!(error = %e, "health check write failed");
            }
            if let Err(e) = stream.shutdown().await {
                tracing::debug!(error = %e, "health check shutdown failed");
            }
        }
    }
}

fn build_relay_pair(
    src: ConnectedChain,
    dst: ConnectedChain,
    relay: &RelayConfig,
) -> eyre::Result<(Arc<dyn DynRelay>, Arc<dyn DynRelay>)> {
    match (src, dst) {
        (ConnectedChain::Cosmos(src_chain), ConnectedChain::Cosmos(dst_chain)) => {
            let src_chain = *src_chain;
            let dst_chain = *dst_chain;
            let src_client_id: ibc::core::host::types::identifiers::ClientId = relay
                .src_client_id
                .parse()
                .map_err(|e| eyre::eyre!("invalid src_client_id '{}': {e}", relay.src_client_id))?;
            let dst_client_id: ibc::core::host::types::identifiers::ClientId = relay
                .dst_client_id
                .parse()
                .map_err(|e| eyre::eyre!("invalid dst_client_id '{}': {e}", relay.dst_client_id))?;

            let fwd: Arc<dyn DynRelay> = Arc::new(RelayContext {
                src_chain: src_chain.clone(),
                dst_chain: dst_chain.clone(),
                src_client_id: src_client_id.clone(),
                dst_client_id: dst_client_id.clone(),
            });
            let rev: Arc<dyn DynRelay> = Arc::new(RelayContext {
                src_chain: dst_chain,
                dst_chain: src_chain,
                src_client_id: dst_client_id,
                dst_client_id: src_client_id,
            });
            Ok((fwd, rev))
        }
        (ConnectedChain::Cosmos(src_chain), ConnectedChain::Ethereum(dst_chain)) => {
            let src_chain = *src_chain;
            let dst_chain = *dst_chain;
            let src_client_id: ibc::core::host::types::identifiers::ClientId = relay
                .src_client_id
                .parse()
                .map_err(|e| eyre::eyre!("invalid src_client_id '{}': {e}", relay.src_client_id))?;
            let dst_client_id = EvmClientId(relay.dst_client_id.clone());

            let fwd: Arc<dyn DynRelay> = Arc::new(RelayContext {
                src_chain: src_chain.clone(),
                dst_chain: dst_chain.clone(),
                src_client_id: src_client_id.clone(),
                dst_client_id: dst_client_id.clone(),
            });
            let rev: Arc<dyn DynRelay> = Arc::new(RelayContext {
                src_chain: dst_chain,
                dst_chain: src_chain,
                src_client_id: dst_client_id,
                dst_client_id: src_client_id,
            });
            Ok((fwd, rev))
        }
        (ConnectedChain::Ethereum(src_chain), ConnectedChain::Cosmos(dst_chain)) => {
            let src_chain = *src_chain;
            let dst_chain = *dst_chain;
            let src_client_id = EvmClientId(relay.src_client_id.clone());
            let dst_client_id: ibc::core::host::types::identifiers::ClientId = relay
                .dst_client_id
                .parse()
                .map_err(|e| eyre::eyre!("invalid dst_client_id '{}': {e}", relay.dst_client_id))?;

            let fwd: Arc<dyn DynRelay> = Arc::new(RelayContext {
                src_chain: src_chain.clone(),
                dst_chain: dst_chain.clone(),
                src_client_id: src_client_id.clone(),
                dst_client_id: dst_client_id.clone(),
            });
            let rev: Arc<dyn DynRelay> = Arc::new(RelayContext {
                src_chain: dst_chain,
                dst_chain: src_chain,
                src_client_id: dst_client_id,
                dst_client_id: src_client_id,
            });
            Ok((fwd, rev))
        }
        (ConnectedChain::Ethereum(_), ConnectedChain::Ethereum(_)) => {
            eyre::bail!("Ethereum-to-Ethereum relay is not supported")
        }
    }
}

fn spawn_relay_pair(
    fwd: Arc<dyn DynRelay>,
    rev: Arc<dyn DynRelay>,
    relay: &RelayConfig,
) -> eyre::Result<JoinHandle<mercury_core::error::Result<()>>> {
    let src_name = relay.src_chain.clone();
    let dst_name = relay.dst_chain.clone();
    let packet_filter = relay
        .packet_filter
        .as_ref()
        .map(PacketFilter::new)
        .transpose()
        .map_err(|e| eyre::eyre!("relay {}->{}: {e}", relay.src_chain, relay.dst_chain))?;

    if let Some(ref pf) = relay.packet_filter {
        tracing::info!(
            policy = ?pf.policy,
            source_ports = ?pf.source_ports,
            "packet filter configured"
        );
    }

    let worker_config = RelayWorkerConfig {
        lookback: relay
            .lookback_window_secs
            .map(std::time::Duration::from_secs),
        clearing_interval: relay
            .clearing_interval_secs
            .map(std::time::Duration::from_secs),
        misbehaviour_scan_interval: relay
            .misbehaviour_scan_interval_secs
            .map(std::time::Duration::from_secs),
        packet_filter,
    };

    let shared_token = tokio_util::sync::CancellationToken::new();

    Ok(tokio::spawn(async move {
        tracing::info!(
            src = %src_name,
            dst = %dst_name,
            "running bidirectional relay"
        );
        let (res_a, res_b) = tokio::join!(
            fwd.run(shared_token.clone(), worker_config.clone()),
            rev.run(shared_token, worker_config),
        );
        if let Err(ref e) = res_a {
            tracing::error!(direction = "a->b", error = %e, "relay direction failed");
        }
        if let Err(ref e) = res_b {
            tracing::error!(direction = "b->a", error = %e, "relay direction failed");
        }
        res_a.and(res_b)
    }))
}
