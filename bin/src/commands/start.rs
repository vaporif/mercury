use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::Args;
use tracing::instrument;
use mercury_core::plugin::{AnyChain, ChainId, DynRelay, DynRelayConfig};
use mercury_core::registry::ChainRegistry;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::LogFormat;
use crate::config::RelayConfig;
use crate::registry::build_registry;

const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(30);

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
    pub async fn run(self, log_format: LogFormat) -> eyre::Result<()> {
        run_start(&self.config, self.health_port, log_format).await
    }
}

struct ChainHandle {
    chain_type: String,
    chain: AnyChain,
}

struct HealthChainEntry {
    chain_id: ChainId,
    chain_type: String,
    chain: AnyChain,
}

#[instrument(skip_all)]
async fn run_start(
    config_path: &Path,
    health_port: Option<u16>,
    log_format: LogFormat,
) -> eyre::Result<()> {
    let registry = build_registry();
    let cfg = crate::config::load_config(config_path, &registry)?;
    let telemetry_guard = mercury_telemetry::init(&cfg.telemetry)?;

    init_subscriber(log_format, &telemetry_guard);

    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));

    for relay in &cfg.relays {
        cfg.find_chain(&registry, &relay.src_chain)
            .map_err(|_| eyre::eyre!("relay references unknown src_chain '{}'", relay.src_chain))?;
        cfg.find_chain(&registry, &relay.dst_chain)
            .map_err(|_| eyre::eyre!("relay references unknown dst_chain '{}'", relay.dst_chain))?;
    }

    let mut chains: HashMap<ChainId, ChainHandle> = HashMap::new();
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

    let shutdown_token = CancellationToken::new();

    let mut handles: Vec<JoinHandle<mercury_core::error::Result<()>>> = Vec::new();
    for relay in &cfg.relays {
        let src = chains
            .get(relay.src_chain.as_str())
            .ok_or_else(|| eyre::eyre!("chain '{}' not in cache", relay.src_chain))?;
        let dst = chains
            .get(relay.dst_chain.as_str())
            .ok_or_else(|| eyre::eyre!("chain '{}' not in cache", relay.dst_chain))?;

        let src_plugin = registry.chain(&src.chain_type)?;
        let dst_plugin = registry.chain(&dst.chain_type)?;
        let src_client_id = src_plugin.parse_client_id(&relay.src_client_id)?;
        let dst_client_id = dst_plugin.parse_client_id(&relay.dst_client_id)?;

        let pair = registry.pair(&src.chain_type, &dst.chain_type)?;
        let (fwd, rev) =
            pair.build_relay(&src.chain, &dst.chain, &src_client_id, &dst_client_id)?;

        let handle = spawn_relay_pair(fwd, rev, relay, shutdown_token.child_token());
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
            .map(|(id, h)| HealthChainEntry {
                chain_id: id,
                chain_type: h.chain_type,
                chain: h.chain,
            })
            .collect();
        tokio::spawn(serve_health(port, registry, health_chains));
    }

    shutdown_signal().await;
    tracing::info!("shutdown signal received, draining in-flight transactions");
    shutdown_token.cancel();

    if tokio::time::timeout(
        SHUTDOWN_GRACE_PERIOD,
        futures::future::join_all(&mut handles),
    )
    .await
    .is_err()
    {
        tracing::warn!("grace period expired, aborting remaining tasks");
        for h in &handles {
            h.abort();
        }
    }

    drop(telemetry_guard);

    Ok(())
}

fn init_subscriber(log_format: LogFormat, telemetry_guard: &mercury_telemetry::TelemetryGuard) {
    let fmt_layer = match log_format {
        LogFormat::Pretty => tracing_subscriber::fmt::layer().with_target(false).boxed(),
        LogFormat::Json => tracing_subscriber::fmt::layer().json().boxed(),
    };

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(fmt_layer);

    if let Some(otel_layer) = telemetry_guard.otel_layer() {
        subscriber.with(otel_layer).init();
    } else {
        subscriber.init();
    }

    if telemetry_guard.is_enabled() {
        tracing::info!("telemetry enabled (OTLP)");
    } else {
        tracing::info!("telemetry disabled (no otlp_endpoint configured)");
    }
}

fn spawn_relay_pair(
    fwd: Arc<dyn DynRelay>,
    rev: Arc<dyn DynRelay>,
    relay: &RelayConfig,
    shared_token: CancellationToken,
) -> JoinHandle<mercury_core::error::Result<()>> {
    let src_name = relay.src_chain.clone();
    let dst_name = relay.dst_chain.clone();

    let config = DynRelayConfig {
        lookback_secs: relay.lookback_window_secs,
        sweep_interval_secs: relay.sweep_interval_secs,
        misbehaviour_scan_interval_secs: relay.misbehaviour_scan_interval_secs,
        packet_filter_config: relay.packet_filter.clone(),
    };

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

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = sigterm.recv() => {}
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to register SIGTERM handler, using ctrl-c only");
                tokio::signal::ctrl_c().await.ok();
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.ok();
    }
}

struct HealthState {
    registry: ChainRegistry,
    chains: Vec<HealthChainEntry>,
}

async fn health_handler(
    axum::extract::State(state): axum::extract::State<Arc<HealthState>>,
) -> axum::response::Json<serde_json::Value> {
    let mut chain_results = serde_json::Map::new();
    let mut all_healthy = true;

    for entry in &state.chains {
        let status = match state.registry.chain(&entry.chain_type) {
            Ok(plugin) => match plugin.query_status(&entry.chain).await {
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
        chain_results.insert(entry.chain_id.to_string(), status);
    }

    axum::response::Json(serde_json::json!({
        "healthy": all_healthy,
        "chains": chain_results,
    }))
}

async fn serve_health(port: u16, registry: ChainRegistry, chains: Vec<HealthChainEntry>) {
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
