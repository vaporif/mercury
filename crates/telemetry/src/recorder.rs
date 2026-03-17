use std::time::{Duration, Instant};

use metrics::{counter, gauge, histogram};

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

/// Metrics for transaction submission workers.
#[derive(Clone)]
pub struct TxMetrics {
    chain: String,
}

impl TxMetrics {
    #[must_use]
    pub const fn new(chain: String) -> Self {
        Self { chain }
    }

    pub fn record_success(
        &self,
        msg_count: usize,
        created_at: Instant,
        confirmed_at: Instant,
        gas_used: Option<u64>,
    ) {
        counter!(metric::tx::TX_SUBMITTED, "chain" => self.chain.clone()).increment(1);
        counter!(metric::tx::TX_MESSAGES, "chain" => self.chain.clone())
            .increment(msg_count as u64);

        histogram!(metric::tx::TX_LATENCY_SUBMITTED_MS, "chain" => self.chain.clone())
            .record(duration_millis(created_at.elapsed()));

        histogram!(metric::tx::TX_LATENCY_CONFIRMED_MS, "chain" => self.chain.clone())
            .record(duration_millis(confirmed_at.duration_since(created_at)));

        if let Some(gas) = gas_used {
            histogram!(metric::tx::GAS_PAID, "chain" => self.chain.clone()).record(u64_gauge(gas));
        }
    }

    pub fn record_error(&self, error: &eyre::Report) {
        let label = error
            .downcast_ref::<TxError>()
            .map_or("unknown", TxError::metric_label);
        counter!(metric::tx::TX_ERRORS, "chain" => self.chain.clone(), "error_type" => label)
            .increment(1);
    }

    pub fn record_consecutive_failures(&self, count: usize) {
        gauge!(metric::tx::TX_CONSECUTIVE_FAILURES, "chain" => self.chain.clone())
            .set(usize_gauge(count));
    }

    pub fn record_channel_utilization(&self, fill: usize) {
        gauge!(metric::tx::TX_CHANNEL_UTILIZATION, "chain" => self.chain.clone())
            .set(usize_gauge(fill));
    }
}

/// Metrics for the packet relay worker.
#[derive(Clone, Default)]
pub struct PacketMetrics;

impl PacketMetrics {
    pub fn record_recv(&self, count: usize) {
        if count > 0 {
            counter!(metric::packet::RECEIVE_PACKETS).increment(count as u64);
        }
    }

    pub fn record_ack(&self, count: usize) {
        if count > 0 {
            counter!(metric::packet::ACK_PACKETS).increment(count as u64);
        }
    }

    pub fn record_timeout(&self, count: usize) {
        if count > 0 {
            counter!(metric::packet::TIMEOUT_PACKETS).increment(count as u64);
        }
    }

    pub fn record_backlog(&self, size: usize) {
        gauge!(metric::backlog::BACKLOG_SIZE).set(usize_gauge(size));
    }
}

/// Metrics for the event watcher worker.
#[derive(Clone, Default)]
pub struct EventMetrics;

impl EventMetrics {
    pub fn record_lag(&self, last_block_at: Instant) {
        gauge!(metric::event::EVENT_WATCHER_LAG_SECS).set(last_block_at.elapsed().as_secs_f64());
    }

    pub fn record_send_events(&self, count: usize) {
        if count > 0 {
            counter!(metric::event::SEND_PACKET_EVENTS).increment(count as u64);
        }
    }

    pub fn record_ack_events(&self, count: usize) {
        if count > 0 {
            counter!(metric::event::ACK_EVENTS).increment(count as u64);
        }
    }

    pub fn record_filtered(&self, count: usize) {
        if count > 0 {
            counter!(metric::packet::FILTERED_PACKETS).increment(count as u64);
        }
    }
}

/// Metrics for the clearing worker.
#[derive(Clone, Default)]
pub struct ClearingMetrics;

impl ClearingMetrics {
    pub fn record_cleared(&self, count: usize) {
        if count > 0 {
            counter!(metric::event::CLEARED_EVENTS).increment(count as u64);
        }
    }
}

/// Metrics for the client refresh worker.
#[derive(Clone, Default)]
pub struct ClientMetrics;

impl ClientMetrics {
    pub fn record_update_submitted(&self) {
        counter!(metric::client::CLIENT_UPDATES_SUBMITTED).increment(1);
    }

    pub fn record_update_skipped(&self) {
        counter!(metric::client::CLIENT_UPDATES_SKIPPED).increment(1);
    }
}

/// Metrics for the misbehaviour worker.
#[derive(Clone, Default)]
pub struct MisbehaviourMetrics;

impl MisbehaviourMetrics {
    pub fn record_submitted(&self) {
        counter!(metric::client::MISBEHAVIOURS_SUBMITTED).increment(1);
    }
}
