use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;
use mercury_core::plugin::{AnyChain, DynRelay, DynRelayConfig};
use mercury_core::registry::ChainRegistry;
use tokio::task::JoinHandle;
use tracing::instrument;

use crate::config::RelayConfig;
use crate::registry::build_registry;

#[derive(Args)]
pub struct StartCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Optional health-check port — serves HTTP endpoints once relays are running
    #[arg(long)]
    health_port: Option<u16>,
}

impl StartCmd {
    pub async fn run(self) -> eyre::Result<()> {
        run_start(&self.config, self.health_port).await
    }
}

struct ChainHandle {
    chain_type: String,
    chain: AnyChain,
}

#[instrument(skip_all, name = "run_start")]
async fn run_start(config_path: &Path, health_port: Option<u16>) -> eyre::Result<()> {
    let registry = build_registry();
    let cfg = crate::config::load_config(config_path, &registry)?;
    mercury_telemetry::init(&cfg.telemetry)?;
    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));

    for relay in &cfg.relays {
        cfg.find_chain(&registry, &relay.src_chain)
            .map_err(|_| eyre::eyre!("relay references unknown src_chain '{}'", relay.src_chain))?;
        cfg.find_chain(&registry, &relay.dst_chain)
            .map_err(|_| eyre::eyre!("relay references unknown dst_chain '{}'", relay.dst_chain))?;
    }

    let mut chains: HashMap<String, ChainHandle> = HashMap::new();
    for chain_cfg in &cfg.chains {
        let plugin = registry.chain(&chain_cfg.chain_type)?;
        let chain = plugin.connect(&chain_cfg.raw, config_dir).await?;
        let id = plugin.chain_id_from_config(&chain_cfg.raw)?;
        tracing::info!(chain_id = %id, chain_type = %chain_cfg.chain_type, "connected to chain");
        chains.insert(
            id,
            ChainHandle {
                chain_type: chain_cfg.chain_type.clone(),
                chain,
            },
        );
    }

    let mut handles: Vec<JoinHandle<mercury_core::error::Result<()>>> = Vec::new();
    for relay in &cfg.relays {
        let src = chains
            .get(&relay.src_chain)
            .ok_or_else(|| eyre::eyre!("chain '{}' not in cache", relay.src_chain))?;
        let dst = chains
            .get(&relay.dst_chain)
            .ok_or_else(|| eyre::eyre!("chain '{}' not in cache", relay.dst_chain))?;

        let pair = registry.pair(&src.chain_type, &dst.chain_type)?;
        let (fwd, rev) = pair.build_relay(
            &src.chain,
            &dst.chain,
            &relay.src_client_id,
            &relay.dst_client_id,
        )?;

        let handle = spawn_relay_pair(fwd, rev, relay);
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
        let health_chains: Vec<_> = chains
            .into_iter()
            .map(|(id, h)| (id, h.chain_type, h.chain))
            .collect();
        tokio::spawn(serve_health(port, registry, health_chains));
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

fn spawn_relay_pair(
    fwd: Arc<dyn DynRelay>,
    rev: Arc<dyn DynRelay>,
    relay: &RelayConfig,
) -> JoinHandle<mercury_core::error::Result<()>> {
    let src_name = relay.src_chain.clone();
    let dst_name = relay.dst_chain.clone();

    let config = DynRelayConfig {
        lookback_secs: relay.lookback_window_secs,
        clearing_interval_secs: relay.clearing_interval_secs,
        misbehaviour_scan_interval_secs: relay.misbehaviour_scan_interval_secs,
        packet_filter_config: relay.packet_filter.clone(),
    };

    let shared_token = tokio_util::sync::CancellationToken::new();

    tokio::spawn(async move {
        tracing::info!(
            src = %src_name,
            dst = %dst_name,
            "running bidirectional relay"
        );
        let (res_a, res_b) = tokio::join!(
            fwd.run(shared_token.clone(), config.clone()),
            rev.run(shared_token, config),
        );
        if let Err(ref e) = res_a {
            tracing::error!(direction = "a->b", error = %e, "relay direction failed");
        }
        if let Err(ref e) = res_b {
            tracing::error!(direction = "b->a", error = %e, "relay direction failed");
        }
        match (res_a, res_b) {
            (Err(a), Err(b)) => Err(eyre::eyre!(
                "{src_name}->{dst_name}: {a}; {dst_name}->{src_name}: {b}"
            )),
            (Err(e), _) | (_, Err(e)) => Err(e),
            _ => Ok(()),
        }
    })
}

struct HealthState {
    registry: ChainRegistry,
    chains: Vec<(String, String, AnyChain)>,
}

async fn health_handler(
    axum::extract::State(state): axum::extract::State<Arc<HealthState>>,
) -> axum::response::Json<serde_json::Value> {
    let mut chain_results = serde_json::Map::new();
    let mut all_healthy = true;

    for (chain_id, chain_type, chain) in &state.chains {
        let status = match state.registry.chain(chain_type) {
            Ok(plugin) => match plugin.query_status(chain).await {
                Ok(info) => serde_json::json!({
                    "status": "healthy",
                    "height": info.height,
                    "timestamp": info.timestamp,
                }),
                Err(e) => {
                    all_healthy = false;
                    serde_json::json!({
                        "status": "unhealthy",
                        "error": e.to_string(),
                    })
                }
            },
            Err(e) => {
                all_healthy = false;
                serde_json::json!({
                    "status": "error",
                    "error": e.to_string(),
                })
            }
        };
        chain_results.insert(chain_id.clone(), status);
    }

    axum::response::Json(serde_json::json!({
        "healthy": all_healthy,
        "chains": chain_results,
    }))
}

async fn serve_health(port: u16, registry: ChainRegistry, chains: Vec<(String, String, AnyChain)>) {
    let state = Arc::new(HealthState { registry, chains });
    let app = axum::Router::new()
        .route("/health", axum::routing::get(health_handler))
        .with_state(state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => {
            tracing::info!(port, "health endpoint listening");
            l
        }
        Err(e) => {
            tracing::error!(port, error = %e, "failed to bind health endpoint");
            return;
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(error = %e, "health server error");
    }
}
