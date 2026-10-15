use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::Args;
use mercury_core::plugin::{AnyChain, ChainId, DynRelay, DynRelayConfig};
use mercury_core::registry::ChainRegistry;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::instrument;
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
        clear_on_start: relay.clear_on_start,
        clear_limit: relay.clear_limit,
        excluded_sequences: relay.excluded_sequences.clone(),
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

#[cfg(test)]
mod tests {
    use mercury_core::plugin::ChainId;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::test_support::{
        HUB, OSMO, ScriptedRelay, StatusPlugin, err_string, example_config, status_chain,
        write_config,
    };

    fn relay_config() -> RelayConfig {
        toml::from_str(
            r#"
src_chain = "a"
dst_chain = "b"
src_client_id = "x"
dst_client_id = "y"
lookback_window_secs = 600
sweep_interval_secs = 30
misbehaviour_scan_interval_secs = 120
clear_on_start = false
clear_limit = 5
excluded_sequences = [3, 7]
packet_filter = { policy = "deny" }
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn relay_pair_runs_both_directions_with_relay_config() {
        let fwd = ScriptedRelay::ok();
        let rev = ScriptedRelay::ok();
        spawn_relay_pair(
            fwd.clone(),
            rev.clone(),
            &relay_config(),
            CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap();

        for relay in [&fwd, &rev] {
            let seen = relay.seen.lock().unwrap().clone().expect("direction ran");
            assert_eq!(seen.lookback_secs, Some(600));
            assert_eq!(seen.sweep_interval_secs, Some(30));
            assert_eq!(seen.misbehaviour_scan_interval_secs, Some(120));
            assert!(!seen.clear_on_start);
            assert_eq!(seen.clear_limit, 5);
            assert_eq!(seen.excluded_sequences, vec![3, 7]);
            let filter = seen.packet_filter_config.unwrap();
            assert_eq!(
                filter.get("policy").and_then(toml::Value::as_str),
                Some("deny")
            );
        }
    }

    #[tokio::test]
    async fn relay_pair_surfaces_single_direction_failure() {
        for (fwd, rev, expected) in [
            (
                ScriptedRelay::failing("fwd broke"),
                ScriptedRelay::ok(),
                "fwd broke",
            ),
            (
                ScriptedRelay::ok(),
                ScriptedRelay::failing("rev broke"),
                "rev broke",
            ),
        ] {
            let err = spawn_relay_pair(fwd, rev, &relay_config(), CancellationToken::new())
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(err.to_string(), expected);
        }
    }

    #[tokio::test]
    async fn relay_pair_reports_both_failures_with_directions() {
        let err = spawn_relay_pair(
            ScriptedRelay::failing("fwd broke"),
            ScriptedRelay::failing("rev broke"),
            &relay_config(),
            CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(err.to_string(), "a->b: fwd broke; b->a: rev broke");
    }

    fn health_state() -> Arc<HealthState> {
        let mut registry = ChainRegistry::new();
        registry.register_chain(StatusPlugin("mock"));
        let entry = |id: &str, chain_type: &str, height| HealthChainEntry {
            chain_id: ChainId::from(id),
            chain_type: chain_type.into(),
            chain: status_chain(height),
        };
        Arc::new(HealthState {
            registry,
            chains: vec![
                entry("up-1", "mock", Some(10)),
                entry("down-1", "mock", None),
                entry("odd-1", "unregistered", Some(1)),
            ],
        })
    }

    #[tokio::test]
    async fn health_reports_each_chain_and_overall_status() {
        let axum::response::Json(body) = health_handler(axum::extract::State(health_state())).await;
        assert_eq!(
            body,
            serde_json::json!({
                "healthy": false,
                "chains": {
                    "up-1": { "status": "healthy", "height": 10, "timestamp": "ts-10" },
                    "down-1": { "status": "unhealthy", "error": "rpc down" },
                    "odd-1": {
                        "status": "error",
                        "error": "unsupported chain type 'unregistered'",
                    },
                },
            })
        );
    }

    #[tokio::test]
    async fn health_is_true_when_every_chain_answers() {
        let mut registry = ChainRegistry::new();
        registry.register_chain(StatusPlugin("mock"));
        let state = Arc::new(HealthState {
            registry,
            chains: vec![HealthChainEntry {
                chain_id: ChainId::from("up-1"),
                chain_type: "mock".into(),
                chain: status_chain(Some(3)),
            }],
        });
        let axum::response::Json(body) = health_handler(axum::extract::State(state)).await;
        assert_eq!(body["healthy"], serde_json::json!(true));
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    async fn get_health(port: u16) -> String {
        let mut stream = loop {
            match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                Ok(s) => break s,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        };
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn serve_health_answers_get_health_with_json() {
        let port = free_port();
        let mut registry = ChainRegistry::new();
        registry.register_chain(StatusPlugin("mock"));
        let chains = vec![HealthChainEntry {
            chain_id: ChainId::from("up-1"),
            chain_type: "mock".into(),
            chain: status_chain(Some(4)),
        }];
        let server = tokio::spawn(serve_health(port, registry, chains));

        let response = tokio::time::timeout(Duration::from_secs(10), get_health(port))
            .await
            .unwrap();
        server.abort();

        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.contains("application/json"), "{response}");
        let body = response.split("\r\n\r\n").nth(1).unwrap();
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["healthy"], serde_json::json!(true));
        assert_eq!(json["chains"]["up-1"]["height"], serde_json::json!(4));
    }

    #[tokio::test]
    async fn serve_health_returns_when_port_is_taken() {
        let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = taken.local_addr().unwrap().port();
        tokio::time::timeout(
            Duration::from_secs(5),
            serve_health(port, ChainRegistry::new(), vec![]),
        )
        .await
        .expect("bind failure must not hang the health task");
    }

    // run_start installs the global tracing subscriber, which can happen once
    // per process; nextest (the project runner) gives each test its own.

    #[tokio::test]
    async fn start_rejects_relay_with_unknown_src_chain() {
        let file = write_config(&format!(
            "{}\n[[relays]]\nsrc_chain = \"juno-1\"\ndst_chain = \"{HUB}\"\nsrc_client_id = \"x\"\ndst_client_id = \"y\"\n",
            crate::test_support::cosmos_chain(HUB)
        ));
        let err = err_string(run_start(&file.path, None, LogFormat::Pretty).await);
        assert_eq!(err, "relay references unknown src_chain 'juno-1'");
    }

    #[tokio::test]
    async fn start_rejects_relay_with_unknown_dst_chain() {
        let file = write_config(&format!(
            "{}\n[[relays]]\nsrc_chain = \"{HUB}\"\ndst_chain = \"juno-1\"\nsrc_client_id = \"x\"\ndst_client_id = \"y\"\n",
            crate::test_support::cosmos_chain(HUB)
        ));
        let err = err_string(run_start(&file.path, None, LogFormat::Json).await);
        assert_eq!(err, "relay references unknown dst_chain 'juno-1'");
    }

    #[tokio::test]
    async fn start_with_nothing_configured_exits_cleanly() {
        let file = write_config("");
        StartCmd {
            config: file.path.clone(),
            health_port: Some(free_port()),
        }
        .run(LogFormat::Pretty)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn start_fails_when_a_chain_cannot_connect() {
        let cfg = example_config();
        let err = err_string(run_start(&cfg.path, None, LogFormat::Pretty).await);
        assert!(
            err.starts_with(&format!("loading signer for '{HUB}'"))
                || err.starts_with(&format!("loading signer for '{OSMO}'")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn start_rejects_invalid_config_before_touching_chains() {
        let file = write_config("[[chains]]\ntype = \"near\"\n");
        let err = err_string(run_start(&file.path, None, LogFormat::Pretty).await);
        assert_eq!(err, "unsupported chain type 'near'");
    }
}
