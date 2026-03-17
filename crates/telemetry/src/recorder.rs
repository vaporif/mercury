use std::sync::Arc;
use std::time::{Duration, Instant};

use metrics::{counter, gauge, histogram};

use mercury_core::ChainLabel;
use mercury_core::error::TxError;

use crate::metric;

fn duration_millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn saturating_gauge(v: impl TryInto<u32>) -> f64 {
    f64::from(v.try_into().unwrap_or(u32::MAX))
}

fn build_labels(
    label: &ChainLabel,
    counterparty: Option<&ChainLabel>,
) -> Vec<(&'static str, String)> {
    let mut labels = label.metric_labels();
    if let Some(cp) = counterparty {
        labels.extend(cp.counterparty_metric_labels());
    }
    labels
}

/// Which chain a tx worker submits transactions to.
#[derive(Clone, Copy, Debug)]
pub enum TxDirection {
    Src,
    Dst,
}

impl TxDirection {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Src => "src",
            Self::Dst => "dst",
        }
    }
}

#[derive(Clone)]
pub struct TxMetrics {
    direction: TxDirection,
    label: ChainLabel,
    counterparty: Option<ChainLabel>,
}

impl TxMetrics {
    #[must_use]
    pub const fn new(direction: TxDirection, label: ChainLabel) -> Self {
        Self {
            direction,
            label,
            counterparty: None,
        }
    }

    #[must_use]
    pub fn with_counterparty(mut self, counterparty: ChainLabel) -> Self {
        self.counterparty = Some(counterparty);
        self
    }

    fn labels(&self) -> Vec<(&'static str, String)> {
        let mut labels = vec![("direction", self.direction.as_str().to_owned())];
        labels.extend(build_labels(&self.label, self.counterparty.as_ref()));
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
            histogram!(metric::tx::GAS_PAID, &labels).record(saturating_gauge(gas));
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
        gauge!(metric::tx::TX_CONSECUTIVE_FAILURES, &labels).set(saturating_gauge(count));
    }

    pub fn record_channel_utilization(&self, fill: usize) {
        let labels = self.labels();
        gauge!(metric::tx::TX_CHANNEL_UTILIZATION, &labels).set(saturating_gauge(fill));
    }
}

#[derive(Clone)]
pub struct PacketMetrics {
    label: ChainLabel,
    counterparty: Option<ChainLabel>,
}

impl PacketMetrics {
    #[must_use]
    pub const fn new(label: ChainLabel) -> Self {
        Self {
            label,
            counterparty: None,
        }
    }

    #[must_use]
    pub fn with_counterparty(mut self, counterparty: ChainLabel) -> Self {
        self.counterparty = Some(counterparty);
        self
    }

    pub fn record_recv(&self, count: usize) {
        if count > 0 {
            let labels = build_labels(&self.label, self.counterparty.as_ref());
            counter!(metric::packet::RECEIVE_PACKETS, &labels).increment(count as u64);
        }
    }

    pub fn record_ack(&self, count: usize) {
        if count > 0 {
            let labels = build_labels(&self.label, self.counterparty.as_ref());
            counter!(metric::packet::ACK_PACKETS, &labels).increment(count as u64);
        }
    }

    pub fn record_timeout(&self, count: usize) {
        if count > 0 {
            let labels = build_labels(&self.label, self.counterparty.as_ref());
            counter!(metric::packet::TIMEOUT_PACKETS, &labels).increment(count as u64);
        }
    }

    pub fn record_backlog(&self, size: usize) {
        let labels = build_labels(&self.label, self.counterparty.as_ref());
        gauge!(metric::backlog::BACKLOG_SIZE, &labels).set(saturating_gauge(size));
    }
}

#[derive(Clone)]
pub struct EventMetrics {
    label: ChainLabel,
    counterparty: Option<ChainLabel>,
}

impl EventMetrics {
    #[must_use]
    pub const fn new(label: ChainLabel) -> Self {
        Self {
            label,
            counterparty: None,
        }
    }

    #[must_use]
    pub fn with_counterparty(mut self, counterparty: ChainLabel) -> Self {
        self.counterparty = Some(counterparty);
        self
    }

    pub fn record_lag(&self, last_block_at: Instant) {
        let labels = build_labels(&self.label, self.counterparty.as_ref());
        gauge!(metric::event::EVENT_WATCHER_LAG_SECS, &labels)
            .set(last_block_at.elapsed().as_secs_f64());
    }

    pub fn record_send_events(&self, count: usize) {
        if count > 0 {
            let labels = build_labels(&self.label, self.counterparty.as_ref());
            counter!(metric::event::SEND_PACKET_EVENTS, &labels).increment(count as u64);
        }
    }

    pub fn record_ack_events(&self, count: usize) {
        if count > 0 {
            let labels = build_labels(&self.label, self.counterparty.as_ref());
            counter!(metric::event::ACK_EVENTS, &labels).increment(count as u64);
        }
    }

    pub fn record_filtered(&self, count: usize) {
        if count > 0 {
            let labels = build_labels(&self.label, self.counterparty.as_ref());
            counter!(metric::packet::FILTERED_PACKETS, &labels).increment(count as u64);
        }
    }
}

#[derive(Clone)]
pub struct SweepMetrics {
    label: ChainLabel,
    counterparty: Option<ChainLabel>,
}

impl SweepMetrics {
    #[must_use]
    pub const fn new(label: ChainLabel) -> Self {
        Self {
            label,
            counterparty: None,
        }
    }

    #[must_use]
    pub fn with_counterparty(mut self, counterparty: ChainLabel) -> Self {
        self.counterparty = Some(counterparty);
        self
    }

    pub fn record_swept(&self, count: usize) {
        if count > 0 {
            let labels = build_labels(&self.label, self.counterparty.as_ref());
            counter!(metric::event::SWEPT_EVENTS, &labels).increment(count as u64);
        }
    }
}

#[derive(Clone)]
pub struct ClientMetrics {
    label: ChainLabel,
    counterparty: Option<ChainLabel>,
    client_id: Option<Arc<str>>,
}

impl ClientMetrics {
    #[must_use]
    pub const fn new(label: ChainLabel) -> Self {
        Self {
            label,
            counterparty: None,
            client_id: None,
        }
    }

    #[must_use]
    pub fn with_counterparty(mut self, counterparty: ChainLabel) -> Self {
        self.counterparty = Some(counterparty);
        self
    }

    #[must_use]
    pub fn with_client_id(mut self, client_id: impl Into<Arc<str>>) -> Self {
        self.client_id = Some(client_id.into());
        self
    }

    fn labels(&self) -> Vec<(&'static str, String)> {
        let mut labels = build_labels(&self.label, self.counterparty.as_ref());
        if let Some(ref id) = self.client_id {
            labels.push(("client_id", id.to_string()));
        }
        labels
    }

    pub fn record_update_submitted(&self) {
        let labels = self.labels();
        counter!(metric::client::CLIENT_UPDATES_SUBMITTED, &labels).increment(1);
    }

    pub fn record_update_skipped(&self) {
        let labels = self.labels();
        counter!(metric::client::CLIENT_UPDATES_SKIPPED, &labels).increment(1);
    }
}

#[derive(Clone)]
pub struct MisbehaviourMetrics {
    label: ChainLabel,
    counterparty: Option<ChainLabel>,
    client_id: Option<Arc<str>>,
}

impl MisbehaviourMetrics {
    #[must_use]
    pub const fn new(label: ChainLabel) -> Self {
        Self {
            label,
            counterparty: None,
            client_id: None,
        }
    }

    #[must_use]
    pub fn with_counterparty(mut self, counterparty: ChainLabel) -> Self {
        self.counterparty = Some(counterparty);
        self
    }

    #[must_use]
    pub fn with_client_id(mut self, client_id: impl Into<Arc<str>>) -> Self {
        self.client_id = Some(client_id.into());
        self
    }

    fn labels(&self) -> Vec<(&'static str, String)> {
        let mut labels = build_labels(&self.label, self.counterparty.as_ref());
        if let Some(ref id) = self.client_id {
            labels.push(("client_id", id.to_string()));
        }
        labels
    }

    pub fn record_submitted(&self) {
        let labels = self.labels();
        counter!(metric::client::MISBEHAVIOURS_SUBMITTED, &labels).increment(1);
    }
}
