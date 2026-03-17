use std::time::{Duration, Instant};

use metrics::{counter, gauge, histogram};

use mercury_core::ChainLabel;
use mercury_core::error::TxError;

use crate::metric;

/// Duration to milliseconds via `as_secs_f64`, avoiding a u128 intermediate.
fn duration_millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Lossless usize-to-f64 via u32 saturation.
/// All metric counts (backlog, channel fill, failure counts) are bounded
/// well below `u32::MAX` by system constraints.
fn usize_gauge(v: usize) -> f64 {
    f64::from(u32::try_from(v).unwrap_or(u32::MAX))
}

/// Lossless u64-to-f64 via u32 saturation.
/// Gas values are bounded by block gas limits (typically < 30M).
fn u64_gauge(v: u64) -> f64 {
    f64::from(u32::try_from(v).unwrap_or(u32::MAX))
}

fn chain_labels(label: &ChainLabel) -> Vec<(&'static str, String)> {
    label
        .metric_labels()
        .into_iter()
        .map(|ml| (ml.name, ml.value))
        .collect()
}

/// Which chain a tx worker submits transactions to.
#[derive(Clone, Copy, Debug)]
pub enum TxDirection {
    Src,
    Dst,
}

impl TxDirection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Src => "src",
            Self::Dst => "dst",
        }
    }
}

/// Metrics for transaction submission workers.
#[derive(Clone)]
pub struct TxMetrics {
    direction: TxDirection,
    label: ChainLabel,
}

impl TxMetrics {
    #[must_use]
    pub const fn new(direction: TxDirection, label: ChainLabel) -> Self {
        Self { direction, label }
    }

    fn labels(&self) -> Vec<(&'static str, String)> {
        let mut labels = vec![("chain", self.direction.as_str().to_owned())];
        labels.extend(chain_labels(&self.label));
        labels
    }

    pub fn record_success(
        &self,
        msg_count: usize,
        created_at: Instant,
        confirmed_at: Instant,
        gas_used: Option<u64>,
    ) {
        let labels = self.labels();
        counter!(metric::tx::TX_SUBMITTED, &labels).increment(1);
        counter!(metric::tx::TX_MESSAGES, &labels).increment(msg_count as u64);

        histogram!(metric::tx::TX_LATENCY_SUBMITTED_MS, &labels)
            .record(duration_millis(created_at.elapsed()));

        histogram!(metric::tx::TX_LATENCY_CONFIRMED_MS, &labels)
            .record(duration_millis(confirmed_at.duration_since(created_at)));

        if let Some(gas) = gas_used {
            histogram!(metric::tx::GAS_PAID, &labels).record(u64_gauge(gas));
        }
    }

    pub fn record_error(&self, error: &eyre::Report) {
        let err_label = error
            .downcast_ref::<TxError>()
            .map_or("unknown", TxError::metric_label);
        let mut labels = self.labels();
        labels.push(("error_type", err_label.to_owned()));
        counter!(metric::tx::TX_ERRORS, &labels).increment(1);
    }

    pub fn record_consecutive_failures(&self, count: usize) {
        let labels = self.labels();
        gauge!(metric::tx::TX_CONSECUTIVE_FAILURES, &labels).set(usize_gauge(count));
    }

    pub fn record_channel_utilization(&self, fill: usize) {
        let labels = self.labels();
        gauge!(metric::tx::TX_CHANNEL_UTILIZATION, &labels).set(usize_gauge(fill));
    }
}

/// Metrics for the packet relay worker.
#[derive(Clone)]
pub struct PacketMetrics {
    label: ChainLabel,
}

impl PacketMetrics {
    #[must_use]
    pub const fn new(label: ChainLabel) -> Self {
        Self { label }
    }

    pub fn record_recv(&self, count: usize) {
        if count > 0 {
            let labels = chain_labels(&self.label);
            counter!(metric::packet::RECEIVE_PACKETS, &labels).increment(count as u64);
        }
    }

    pub fn record_ack(&self, count: usize) {
        if count > 0 {
            let labels = chain_labels(&self.label);
            counter!(metric::packet::ACK_PACKETS, &labels).increment(count as u64);
        }
    }

    pub fn record_timeout(&self, count: usize) {
        if count > 0 {
            let labels = chain_labels(&self.label);
            counter!(metric::packet::TIMEOUT_PACKETS, &labels).increment(count as u64);
        }
    }

    pub fn record_backlog(&self, size: usize) {
        let labels = chain_labels(&self.label);
        gauge!(metric::backlog::BACKLOG_SIZE, &labels).set(usize_gauge(size));
    }
}

/// Metrics for the event watcher worker.
#[derive(Clone)]
pub struct EventMetrics {
    label: ChainLabel,
}

impl EventMetrics {
    #[must_use]
    pub const fn new(label: ChainLabel) -> Self {
        Self { label }
    }

    pub fn record_lag(&self, last_block_at: Instant) {
        let labels = chain_labels(&self.label);
        gauge!(metric::event::EVENT_WATCHER_LAG_SECS, &labels)
            .set(last_block_at.elapsed().as_secs_f64());
    }

    pub fn record_send_events(&self, count: usize) {
        if count > 0 {
            let labels = chain_labels(&self.label);
            counter!(metric::event::SEND_PACKET_EVENTS, &labels).increment(count as u64);
        }
    }

    pub fn record_ack_events(&self, count: usize) {
        if count > 0 {
            let labels = chain_labels(&self.label);
            counter!(metric::event::ACK_EVENTS, &labels).increment(count as u64);
        }
    }

    pub fn record_filtered(&self, count: usize) {
        if count > 0 {
            let labels = chain_labels(&self.label);
            counter!(metric::packet::FILTERED_PACKETS, &labels).increment(count as u64);
        }
    }
}

/// Metrics for the clearing worker.
#[derive(Clone)]
pub struct ClearingMetrics {
    label: ChainLabel,
}

impl ClearingMetrics {
    #[must_use]
    pub const fn new(label: ChainLabel) -> Self {
        Self { label }
    }

    pub fn record_cleared(&self, count: usize) {
        if count > 0 {
            let labels = chain_labels(&self.label);
            counter!(metric::event::CLEARED_EVENTS, &labels).increment(count as u64);
        }
    }
}

/// Metrics for the client refresh worker.
#[derive(Clone)]
pub struct ClientMetrics {
    label: ChainLabel,
}

impl ClientMetrics {
    #[must_use]
    pub const fn new(label: ChainLabel) -> Self {
        Self { label }
    }

    pub fn record_update_submitted(&self) {
        let labels = chain_labels(&self.label);
        counter!(metric::client::CLIENT_UPDATES_SUBMITTED, &labels).increment(1);
    }

    pub fn record_update_skipped(&self) {
        let labels = chain_labels(&self.label);
        counter!(metric::client::CLIENT_UPDATES_SKIPPED, &labels).increment(1);
    }
}

/// Metrics for the misbehaviour worker.
#[derive(Clone)]
pub struct MisbehaviourMetrics {
    label: ChainLabel,
}

impl MisbehaviourMetrics {
    #[must_use]
    pub const fn new(label: ChainLabel) -> Self {
        Self { label }
    }

    pub fn record_submitted(&self) {
        let labels = chain_labels(&self.label);
        counter!(metric::client::MISBEHAVIOURS_SUBMITTED, &labels).increment(1);
    }
}
