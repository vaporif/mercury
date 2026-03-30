use std::sync::Arc;
use std::time::{Duration, Instant};

use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};
use opentelemetry::{KeyValue, global};

use mercury_core::ChainLabel;
use mercury_core::error::TxError;

use crate::metric;

fn duration_millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn saturating_gauge(v: impl TryInto<u32>) -> f64 {
    f64::from(v.try_into().unwrap_or(u32::MAX))
}

#[allow(clippy::cast_possible_truncation)]
const fn count_as_u64(v: usize) -> u64 {
    v as u64
}

fn build_attributes(label: &ChainLabel, counterparty: Option<&ChainLabel>) -> Vec<KeyValue> {
    let mut attrs: Vec<KeyValue> = label
        .metric_labels()
        .into_iter()
        .map(|(k, v)| KeyValue::new(k, v))
        .collect();
    if let Some(cp) = counterparty {
        for (k, v) in cp.counterparty_metric_labels() {
            attrs.push(KeyValue::new(k, v));
        }
    }
    attrs
}

fn meter() -> Meter {
    global::meter("mercury_telemetry")
}

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
    cached_attrs: Vec<KeyValue>,
    tx_submitted: Counter<u64>,
    tx_messages: Counter<u64>,
    tx_latency_submitted: Histogram<f64>,
    tx_latency_confirmed: Histogram<f64>,
    gas_paid: Histogram<f64>,
    tx_errors: Counter<u64>,
    tx_consecutive_failures: Gauge<f64>,
    tx_channel_utilization: Gauge<f64>,
}

impl TxMetrics {
    #[must_use]
    pub fn new(direction: TxDirection, label: &ChainLabel) -> Self {
        let m = meter();
        let mut cached_attrs = vec![KeyValue::new("direction", direction.as_str())];
        cached_attrs.extend(build_attributes(label, None));
        Self {
            cached_attrs,
            tx_submitted: m.u64_counter(metric::tx::TX_SUBMITTED).build(),
            tx_messages: m.u64_counter(metric::tx::TX_MESSAGES).build(),
            tx_latency_submitted: m.f64_histogram(metric::tx::TX_LATENCY_SUBMITTED_MS).build(),
            tx_latency_confirmed: m.f64_histogram(metric::tx::TX_LATENCY_CONFIRMED_MS).build(),
            gas_paid: m.f64_histogram(metric::tx::GAS_PAID).build(),
            tx_errors: m.u64_counter(metric::tx::TX_ERRORS).build(),
            tx_consecutive_failures: m.f64_gauge(metric::tx::TX_CONSECUTIVE_FAILURES).build(),
            tx_channel_utilization: m.f64_gauge(metric::tx::TX_CHANNEL_UTILIZATION).build(),
        }
    }

    #[must_use]
    pub fn with_counterparty(mut self, counterparty: &ChainLabel) -> Self {
        for (k, v) in counterparty.counterparty_metric_labels() {
            self.cached_attrs.push(KeyValue::new(k, v));
        }
        self
    }

    pub fn record_success(
        &self,
        msg_count: usize,
        created_at: Instant,
        confirmed_at: Instant,
        gas_used: Option<u64>,
    ) {
        self.tx_submitted.add(1, &self.cached_attrs);
        self.tx_messages
            .add(count_as_u64(msg_count), &self.cached_attrs);
        self.tx_latency_submitted
            .record(duration_millis(created_at.elapsed()), &self.cached_attrs);
        self.tx_latency_confirmed.record(
            duration_millis(confirmed_at.duration_since(created_at)),
            &self.cached_attrs,
        );
        if let Some(gas) = gas_used {
            self.gas_paid
                .record(saturating_gauge(gas), &self.cached_attrs);
        }
    }

    pub fn record_error(&self, error: &eyre::Report) {
        let err_label = error
            .downcast_ref::<TxError>()
            .map_or("unknown", TxError::metric_label);
        let mut attrs = self.cached_attrs.clone();
        attrs.push(KeyValue::new("error_type", err_label.to_owned()));
        self.tx_errors.add(1, &attrs);
    }

    pub fn record_consecutive_failures(&self, count: usize) {
        self.tx_consecutive_failures
            .record(saturating_gauge(count), &self.cached_attrs);
    }

    pub fn record_channel_utilization(&self, fill: usize) {
        self.tx_channel_utilization
            .record(saturating_gauge(fill), &self.cached_attrs);
    }
}

#[derive(Clone)]
pub struct PacketMetrics {
    cached_attrs: Vec<KeyValue>,
    receive_packets: Counter<u64>,
    ack_packets: Counter<u64>,
    timeout_packets: Counter<u64>,
    backlog_size: Gauge<f64>,
    backlog_oldest_sequence: Gauge<f64>,
}

impl PacketMetrics {
    #[must_use]
    pub fn new(label: &ChainLabel) -> Self {
        let m = meter();
        Self {
            cached_attrs: build_attributes(label, None),
            receive_packets: m.u64_counter(metric::packet::RECEIVE_PACKETS).build(),
            ack_packets: m.u64_counter(metric::packet::ACK_PACKETS).build(),
            timeout_packets: m.u64_counter(metric::packet::TIMEOUT_PACKETS).build(),
            backlog_size: m.f64_gauge(metric::backlog::BACKLOG_SIZE).build(),
            backlog_oldest_sequence: m
                .f64_gauge(metric::backlog::BACKLOG_OLDEST_SEQUENCE)
                .build(),
        }
    }

    #[must_use]
    pub fn with_counterparty(mut self, counterparty: &ChainLabel) -> Self {
        for (k, v) in counterparty.counterparty_metric_labels() {
            self.cached_attrs.push(KeyValue::new(k, v));
        }
        self
    }

    pub fn record_recv(&self, count: usize) {
        if count > 0 {
            self.receive_packets
                .add(count_as_u64(count), &self.cached_attrs);
        }
    }

    pub fn record_ack(&self, count: usize) {
        if count > 0 {
            self.ack_packets
                .add(count_as_u64(count), &self.cached_attrs);
        }
    }

    pub fn record_timeout(&self, count: usize) {
        if count > 0 {
            self.timeout_packets
                .add(count_as_u64(count), &self.cached_attrs);
        }
    }

    pub fn record_backlog(&self, size: usize) {
        self.backlog_size
            .record(saturating_gauge(size), &self.cached_attrs);
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn record_oldest_sequence(&self, sequence: u64) {
        self.backlog_oldest_sequence
            .record(sequence as f64, &self.cached_attrs);
    }
}

#[derive(Clone)]
pub struct EventMetrics {
    cached_attrs: Vec<KeyValue>,
    send_packet_events: Counter<u64>,
    ack_events: Counter<u64>,
    filtered_packets: Counter<u64>,
    event_watcher_lag: Gauge<f64>,
    event_source_mode: Gauge<f64>,
    ws_reconnect_total: Counter<u64>,
    ws_reconnect_failed: Counter<u64>,
    ws_events_received: Counter<u64>,
    ws_last_connect_ts: Gauge<f64>,
    ws_fallback_total: Counter<u64>,
}

impl EventMetrics {
    #[must_use]
    pub fn new(label: &ChainLabel) -> Self {
        let m = meter();
        Self {
            cached_attrs: build_attributes(label, None),
            send_packet_events: m.u64_counter(metric::event::SEND_PACKET_EVENTS).build(),
            ack_events: m.u64_counter(metric::event::ACK_EVENTS).build(),
            filtered_packets: m.u64_counter(metric::packet::FILTERED_PACKETS).build(),
            event_watcher_lag: m.f64_gauge(metric::event::EVENT_WATCHER_LAG_SECS).build(),
            event_source_mode: m.f64_gauge(metric::event::EVENT_SOURCE_MODE).build(),
            ws_reconnect_total: m.u64_counter(metric::event::WS_RECONNECT_TOTAL).build(),
            ws_reconnect_failed: m
                .u64_counter(metric::event::WS_RECONNECT_FAILED_TOTAL)
                .build(),
            ws_events_received: m
                .u64_counter(metric::event::WS_EVENTS_RECEIVED_TOTAL)
                .build(),
            ws_last_connect_ts: m
                .f64_gauge(metric::event::WS_LAST_CONNECT_TIMESTAMP_SECONDS)
                .build(),
            ws_fallback_total: m.u64_counter(metric::event::WS_FALLBACK_TOTAL).build(),
        }
    }

    #[must_use]
    pub fn with_counterparty(mut self, counterparty: &ChainLabel) -> Self {
        for (k, v) in counterparty.counterparty_metric_labels() {
            self.cached_attrs.push(KeyValue::new(k, v));
        }
        self
    }

    pub fn record_lag(&self, last_block_at: Instant) {
        self.event_watcher_lag
            .record(last_block_at.elapsed().as_secs_f64(), &self.cached_attrs);
    }

    pub fn record_send_events(&self, count: usize) {
        if count > 0 {
            self.send_packet_events
                .add(count_as_u64(count), &self.cached_attrs);
        }
    }

    pub fn record_ack_events(&self, count: usize) {
        if count > 0 {
            self.ack_events.add(count_as_u64(count), &self.cached_attrs);
        }
    }

    pub fn record_filtered(&self, count: usize) {
        if count > 0 {
            self.filtered_packets
                .add(count_as_u64(count), &self.cached_attrs);
        }
    }

    pub fn record_event_source_mode(&self, websocket: bool) {
        self.event_source_mode
            .record(if websocket { 1.0 } else { 0.0 }, &self.cached_attrs);
    }

    pub fn record_ws_reconnect_attempt(&self) {
        self.ws_reconnect_total.add(1, &self.cached_attrs);
    }

    pub fn record_ws_reconnect_failed(&self) {
        self.ws_reconnect_failed.add(1, &self.cached_attrs);
    }

    pub fn record_ws_events(&self, count: usize) {
        if count > 0 {
            self.ws_events_received
                .add(count_as_u64(count), &self.cached_attrs);
        }
    }

    pub fn record_ws_connected(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        self.ws_last_connect_ts.record(now, &self.cached_attrs);
    }

    pub fn record_ws_fallback(&self) {
        self.ws_fallback_total.add(1, &self.cached_attrs);
    }
}

#[derive(Clone)]
pub struct SweepMetrics {
    cached_attrs: Vec<KeyValue>,
    swept_events: Counter<u64>,
    recv_cleared: Counter<u64>,
    ack_cleared: Counter<u64>,
    excluded: Counter<u64>,
    scan_duration: Histogram<f64>,
    errors: Counter<u64>,
}

impl SweepMetrics {
    #[must_use]
    pub fn new(label: &ChainLabel) -> Self {
        let m = meter();
        Self {
            cached_attrs: build_attributes(label, None),
            swept_events: m.u64_counter(metric::sweep::SWEPT_EVENTS).build(),
            recv_cleared: m.u64_counter(metric::sweep::SWEEP_RECV_CLEARED).build(),
            ack_cleared: m.u64_counter(metric::sweep::SWEEP_ACK_CLEARED).build(),
            excluded: m.u64_counter(metric::sweep::SWEEP_EXCLUDED).build(),
            scan_duration: m
                .f64_histogram(metric::sweep::SWEEP_SCAN_DURATION_SECONDS)
                .build(),
            errors: m.u64_counter(metric::sweep::SWEEP_ERRORS).build(),
        }
    }

    #[must_use]
    pub fn with_counterparty(mut self, counterparty: &ChainLabel) -> Self {
        for (k, v) in counterparty.counterparty_metric_labels() {
            self.cached_attrs.push(KeyValue::new(k, v));
        }
        self
    }

    pub fn record_swept(&self, count: usize) {
        if count > 0 {
            self.swept_events
                .add(count_as_u64(count), &self.cached_attrs);
        }
    }

    pub fn record_recv_cleared(&self, count: usize) {
        if count > 0 {
            self.recv_cleared
                .add(count_as_u64(count), &self.cached_attrs);
        }
    }

    pub fn record_ack_cleared(&self, count: usize) {
        if count > 0 {
            self.ack_cleared
                .add(count_as_u64(count), &self.cached_attrs);
        }
    }

    pub fn record_excluded(&self, count: usize) {
        if count > 0 {
            self.excluded.add(count_as_u64(count), &self.cached_attrs);
        }
    }

    pub fn record_scan_duration(&self, duration: Duration) {
        self.scan_duration
            .record(duration.as_secs_f64(), &self.cached_attrs);
    }

    pub fn record_error(&self, phase: &str) {
        let mut attrs = self.cached_attrs.clone();
        attrs.push(KeyValue::new("phase", phase.to_owned()));
        self.errors.add(1, &attrs);
    }
}
#[derive(Clone)]
pub struct ClientMetrics {
    cached_attrs: Vec<KeyValue>,
    updates_submitted: Counter<u64>,
    updates_skipped: Counter<u64>,
}

impl ClientMetrics {
    #[must_use]
    pub fn new(label: &ChainLabel) -> Self {
        let m = meter();
        Self {
            cached_attrs: build_attributes(label, None),
            updates_submitted: m
                .u64_counter(metric::client::CLIENT_UPDATES_SUBMITTED)
                .build(),
            updates_skipped: m
                .u64_counter(metric::client::CLIENT_UPDATES_SKIPPED)
                .build(),
        }
    }

    #[must_use]
    pub fn with_counterparty(mut self, counterparty: &ChainLabel) -> Self {
        for (k, v) in counterparty.counterparty_metric_labels() {
            self.cached_attrs.push(KeyValue::new(k, v));
        }
        self
    }

    #[must_use]
    pub fn with_client_id(mut self, client_id: impl Into<Arc<str>>) -> Self {
        self.cached_attrs
            .push(KeyValue::new("client_id", client_id.into().to_string()));
        self
    }

    pub fn record_update_submitted(&self) {
        self.updates_submitted.add(1, &self.cached_attrs);
    }

    pub fn record_update_skipped(&self) {
        self.updates_skipped.add(1, &self.cached_attrs);
    }
}

#[derive(Clone)]
pub struct MisbehaviourMetrics {
    cached_attrs: Vec<KeyValue>,
    submitted: Counter<u64>,
}

impl MisbehaviourMetrics {
    #[must_use]
    pub fn new(label: &ChainLabel) -> Self {
        let m = meter();
        Self {
            cached_attrs: build_attributes(label, None),
            submitted: m
                .u64_counter(metric::client::MISBEHAVIOURS_SUBMITTED)
                .build(),
        }
    }

    #[must_use]
    pub fn with_counterparty(mut self, counterparty: &ChainLabel) -> Self {
        for (k, v) in counterparty.counterparty_metric_labels() {
            self.cached_attrs.push(KeyValue::new(k, v));
        }
        self
    }

    #[must_use]
    pub fn with_client_id(mut self, client_id: impl Into<Arc<str>>) -> Self {
        self.cached_attrs
            .push(KeyValue::new("client_id", client_id.into().to_string()));
        self
    }

    pub fn record_submitted(&self) {
        self.submitted.add(1, &self.cached_attrs);
    }
}

// TODO: wire when wallet balance worker is implemented
#[allow(dead_code)]
#[derive(Clone)]
pub struct WalletMetrics {
    cached_attrs: Vec<KeyValue>,
    wallet_balance: Gauge<f64>,
}

#[allow(dead_code)]
impl WalletMetrics {
    #[must_use]
    pub fn new(label: &ChainLabel) -> Self {
        let m = meter();
        Self {
            cached_attrs: build_attributes(label, None),
            wallet_balance: m.f64_gauge(metric::wallet::WALLET_BALANCE).build(),
        }
    }

    pub fn record_balance(&self, amount: f64, account: &str, denom: &str) {
        let mut attrs = self.cached_attrs.clone();
        attrs.push(KeyValue::new("account", account.to_owned()));
        attrs.push(KeyValue::new("denom", denom.to_owned()));
        self.wallet_balance.record(amount, &attrs);
    }
}

#[derive(Clone)]
pub struct GasMetrics {
    cached_attrs: Vec<KeyValue>,
    gas_price_gwei: Histogram<f64>,
}

impl std::fmt::Debug for GasMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GasMetrics").finish_non_exhaustive()
    }
}

impl GasMetrics {
    #[must_use]
    pub fn new(label: &ChainLabel) -> Self {
        let m = meter();
        Self {
            cached_attrs: build_attributes(label, None),
            gas_price_gwei: m.f64_histogram(metric::gas::GAS_PRICE_GWEI).build(),
        }
    }

    pub fn record_gas_price(&self, price: f64) {
        self.gas_price_gwei.record(price, &self.cached_attrs);
    }
}
