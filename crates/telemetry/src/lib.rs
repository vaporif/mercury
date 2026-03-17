use std::net::IpAddr;

use eyre::Context;
use metrics::{describe_counter, describe_gauge, describe_histogram};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
use serde::Deserialize;

pub mod guard;
pub mod metric;
pub mod recorder;

#[derive(Debug, Clone, Deserialize)]
pub struct TelemetryConfig {
    #[serde(default)]
    pub metrics_port: Option<u16>,
    #[serde(default = "default_metrics_host")]
    pub metrics_host: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            metrics_port: None,
            metrics_host: default_metrics_host(),
        }
    }
}

fn default_metrics_host() -> String {
    "127.0.0.1".to_string()
}

pub fn init(config: &TelemetryConfig) -> eyre::Result<()> {
    let Some(port) = config.metrics_port else {
        tracing::info!("telemetry disabled (no metrics_port configured)");
        return Ok(());
    };

    let host: IpAddr = config
        .metrics_host
        .parse()
        .wrap_err("invalid metrics_host")?;

    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Suffix("latency_submitted_ms".to_string()),
            &metric::tx::TX_LATENCY_SUBMITTED_BUCKETS,
        )?
        .set_buckets_for_metric(
            Matcher::Suffix("latency_confirmed_ms".to_string()),
            &metric::tx::TX_LATENCY_CONFIRMED_BUCKETS,
        )?
        .set_buckets_for_metric(
            Matcher::Suffix("query_latency_ms".to_string()),
            &metric::query::QUERY_LATENCY_BUCKETS,
        )?
        .set_buckets_for_metric(
            Matcher::Suffix("gas_paid".to_string()),
            &metric::tx::GAS_PAID_BUCKETS,
        )?
        .set_buckets_for_metric(
            Matcher::Suffix("gas_price_gwei".to_string()),
            &metric::gas::GAS_PRICE_GWEI_BUCKETS,
        )?
        .with_http_listener((host, port))
        .install()
        .wrap_err("failed to install Prometheus exporter")?;

    register();

    let process_collector = metrics_process::Collector::default();
    process_collector.describe();
    process_collector.collect();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
        loop {
            interval.tick().await;
            process_collector.collect();
        }
    });

    tracing::info!(port, %host, "telemetry enabled");
    Ok(())
}

fn register() {
    // Packet
    describe_counter!(metric::packet::RECEIVE_PACKETS, "Receive packets relayed");
    describe_counter!(
        metric::packet::ACK_PACKETS,
        "Acknowledgement packets relayed"
    );
    describe_counter!(metric::packet::TIMEOUT_PACKETS, "Timeout packets relayed");
    describe_counter!(
        metric::packet::FILTERED_PACKETS,
        "Packets rejected by port filter"
    );

    // TX
    describe_counter!(metric::tx::TX_SUBMITTED, "Transactions submitted");
    describe_counter!(metric::tx::TX_MESSAGES, "Messages included in transactions");
    describe_histogram!(
        metric::tx::TX_LATENCY_SUBMITTED_MS,
        "Batch creation to TX broadcast (ms)"
    );
    describe_histogram!(
        metric::tx::TX_LATENCY_CONFIRMED_MS,
        "Batch creation to TX confirmed (ms)"
    );
    describe_counter!(metric::tx::TX_ERRORS, "Transaction errors");
    describe_gauge!(
        metric::tx::TX_CONSECUTIVE_FAILURES,
        "Current consecutive TX failure count"
    );
    describe_histogram!(metric::tx::GAS_PAID, "Gas consumed per transaction");
    describe_gauge!(
        metric::tx::TX_CHANNEL_UTILIZATION,
        "TX channel buffer fill level"
    );

    // Events
    describe_counter!(
        metric::event::SEND_PACKET_EVENTS,
        "SendPacket events extracted"
    );
    describe_counter!(metric::event::ACK_EVENTS, "WriteAck events extracted");
    describe_counter!(
        metric::event::CLEARED_EVENTS,
        "Packets found during clearing scan"
    );
    describe_gauge!(
        metric::event::EVENT_WATCHER_LAG_SECS,
        "Seconds since last block processed"
    );

    // Client
    describe_counter!(
        metric::client::CLIENT_UPDATES_SUBMITTED,
        "Client updates submitted"
    );
    describe_counter!(
        metric::client::CLIENT_UPDATES_SKIPPED,
        "Client updates skipped (already current)"
    );
    describe_counter!(
        metric::client::MISBEHAVIOURS_SUBMITTED,
        "Misbehaviour evidence submitted"
    );

    // Workers
    describe_gauge!(metric::worker::WORKERS, "Active worker count by type");
    describe_counter!(metric::worker::WORKER_ERRORS, "Worker task failures");

    // Proofs
    describe_counter!(metric::proof::PROOF_FETCH_RETRIES, "Proof fetch retries");
    describe_counter!(
        metric::proof::PROOF_FETCH_FAILURES,
        "Proof fetch total failures"
    );

    // Queries
    describe_counter!(metric::query::QUERIES, "Chain queries");
    describe_histogram!(metric::query::QUERY_LATENCY_MS, "Chain query latency (ms)");

    // Backlog
    describe_gauge!(metric::backlog::BACKLOG_SIZE, "Pending packet backlog size");
    describe_gauge!(
        metric::backlog::BACKLOG_OLDEST_SEQUENCE,
        "Oldest pending packet sequence"
    );

    // Wallet
    describe_gauge!(metric::wallet::WALLET_BALANCE, "Relayer wallet balance");

    // Gas
    describe_histogram!(metric::gas::GAS_PRICE_GWEI, "Gas price in gwei at TX time");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_no_port() {
        let config = TelemetryConfig::default();
        assert!(config.metrics_port.is_none());
        assert_eq!(config.metrics_host, "127.0.0.1");
    }

    #[test]
    fn init_noop_when_disabled() {
        let config = TelemetryConfig::default();
        assert!(init(&config).is_ok());
    }

    #[test]
    fn deserialize_from_toml() {
        let toml_str = r#"
            metrics_port = 9090
            metrics_host = "0.0.0.0"
        "#;
        let config: TelemetryConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.metrics_port, Some(9090));
        assert_eq!(config.metrics_host, "0.0.0.0");
    }

    #[test]
    fn deserialize_minimal_toml() {
        let toml_str = "";
        let config: TelemetryConfig = toml::from_str(toml_str).unwrap();
        assert!(config.metrics_port.is_none());
    }
}
