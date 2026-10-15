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

    #[expect(clippy::cast_precision_loss, reason = "values stay well below 2^53")]
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
#[derive(Clone)]
pub struct WalletMetrics {
    cached_attrs: Vec<KeyValue>,
    wallet_balance: Gauge<f64>,
}

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

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::OnceLock;

    use opentelemetry_sdk::metrics::data::{
        AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics,
    };
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

    use super::*;

    #[derive(Debug, PartialEq)]
    enum Point {
        U64(u64),
        F64(f64),
        Hist { count: u64, sum: f64 },
    }

    /// One process-wide provider: the global meter can only be set once, so
    /// each test isolates its data by using a unique chain name.
    pub fn harness() -> &'static (SdkMeterProvider, InMemoryMetricExporter) {
        static HARNESS: OnceLock<(SdkMeterProvider, InMemoryMetricExporter)> = OnceLock::new();
        HARNESS.get_or_init(|| {
            let exporter = InMemoryMetricExporter::default();
            let provider = SdkMeterProvider::builder()
                .with_periodic_exporter(exporter.clone())
                .build();
            global::set_meter_provider(provider.clone());
            (provider, exporter)
        })
    }

    fn label(name: &str) -> ChainLabel {
        harness();
        ChainLabel::new(name)
    }

    fn attr(attrs: &[KeyValue], key: &str) -> Option<String> {
        attrs
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.as_str().into_owned())
    }

    fn points_in(rm: &ResourceMetrics, chain: &str, name: &str) -> Vec<(Vec<KeyValue>, Point)> {
        let mut out = Vec::new();
        let ours = |attrs: &[KeyValue]| attr(attrs, "chain_name").as_deref() == Some(chain);
        for metric in rm.scope_metrics().flat_map(ScopeMetrics::metrics) {
            if metric.name() != name {
                continue;
            }
            match metric.data() {
                AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                    for dp in sum.data_points() {
                        let attrs: Vec<KeyValue> = dp.attributes().cloned().collect();
                        if ours(&attrs) {
                            out.push((attrs, Point::U64(dp.value())));
                        }
                    }
                }
                AggregatedMetrics::F64(MetricData::Gauge(gauge)) => {
                    for dp in gauge.data_points() {
                        let attrs: Vec<KeyValue> = dp.attributes().cloned().collect();
                        if ours(&attrs) {
                            out.push((attrs, Point::F64(dp.value())));
                        }
                    }
                }
                AggregatedMetrics::F64(MetricData::Histogram(hist)) => {
                    for dp in hist.data_points() {
                        let attrs: Vec<KeyValue> = dp.attributes().cloned().collect();
                        if ours(&attrs) {
                            out.push((
                                attrs,
                                Point::Hist {
                                    count: dp.count(),
                                    sum: dp.sum(),
                                },
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Flushes and returns the newest data points of `name` for `chain`.
    fn collect(chain: &str, name: &str) -> Vec<(Vec<KeyValue>, Point)> {
        let (provider, exporter) = harness();
        provider.force_flush().unwrap();
        let exports = exporter.get_finished_metrics().unwrap();
        exports
            .iter()
            .rev()
            .map(|rm| points_in(rm, chain, name))
            .find(|points| !points.is_empty())
            .unwrap_or_default()
    }

    /// The single point of `name` for `chain`, panicking if absent or ambiguous.
    fn single(chain: &str, name: &str) -> (Vec<KeyValue>, Point) {
        let mut points = collect(chain, name);
        assert_eq!(points.len(), 1, "expected one {name} point, got {points:?}");
        points.remove(0)
    }

    fn value(chain: &str, name: &str) -> Point {
        single(chain, name).1
    }

    #[test]
    fn saturating_gauge_clamps_to_u32() {
        assert!((saturating_gauge(7u64) - 7.0).abs() < f64::EPSILON);
        assert!((saturating_gauge(u64::MAX) - f64::from(u32::MAX)).abs() < f64::EPSILON);
        assert!((duration_millis(Duration::from_micros(1500)) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn tx_success_records_counts_latency_and_gas() {
        let chain = "tx-success";
        let metrics = TxMetrics::new(TxDirection::Dst, &label(chain))
            .with_counterparty(&ChainLabel::new("cp"));
        let created = Instant::now();
        metrics.record_success(3, created, created + Duration::from_millis(250), Some(5));

        let (attrs, submitted) = single(chain, metric::tx::TX_SUBMITTED);
        assert_eq!(submitted, Point::U64(1));
        assert_eq!(attr(&attrs, "direction").as_deref(), Some("dst"));
        assert_eq!(
            attr(&attrs, "counterparty_chain_name").as_deref(),
            Some("cp")
        );
        assert_eq!(value(chain, metric::tx::TX_MESSAGES), Point::U64(3));
        assert_eq!(
            value(chain, metric::tx::TX_LATENCY_CONFIRMED_MS),
            Point::Hist {
                count: 1,
                sum: 250.0
            }
        );
        assert!(matches!(
            value(chain, metric::tx::TX_LATENCY_SUBMITTED_MS),
            Point::Hist { count: 1, .. }
        ));
        assert_eq!(
            value(chain, metric::tx::GAS_PAID),
            Point::Hist { count: 1, sum: 5.0 }
        );
    }

    #[test]
    fn tx_success_without_gas_skips_gas_histogram() {
        let chain = "tx-no-gas";
        let metrics = TxMetrics::new(TxDirection::Src, &label(chain));
        let now = Instant::now();
        metrics.record_success(1, now, now, None);

        let (attrs, _) = single(chain, metric::tx::TX_SUBMITTED);
        assert_eq!(attr(&attrs, "direction").as_deref(), Some("src"));
        assert_eq!(collect(chain, metric::tx::GAS_PAID), vec![]);
    }

    #[test]
    fn tx_error_is_labelled_by_type() {
        let chain = "tx-errors";
        let metrics = TxMetrics::new(TxDirection::Dst, &label(chain));
        let out_of_gas = TxError::OutOfGas {
            details: "x".into(),
        };
        let expected = out_of_gas.metric_label();
        metrics.record_error(&eyre::Report::new(out_of_gas));
        metrics.record_error(&eyre::eyre!("opaque"));
        metrics.record_error(&eyre::eyre!("opaque again"));

        let mut by_type: Vec<(String, Point)> = collect(chain, metric::tx::TX_ERRORS)
            .into_iter()
            .map(|(attrs, p)| (attr(&attrs, "error_type").unwrap(), p))
            .collect();
        by_type.sort_by(|a, b| a.0.cmp(&b.0));
        let mut want = vec![
            (expected.to_owned(), Point::U64(1)),
            ("unknown".to_owned(), Point::U64(2)),
        ];
        want.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(by_type, want);
    }

    #[test]
    fn tx_gauges_keep_latest_value() {
        let chain = "tx-gauges";
        let metrics = TxMetrics::new(TxDirection::Dst, &label(chain));
        metrics.record_consecutive_failures(4);
        metrics.record_consecutive_failures(2);
        metrics.record_channel_utilization(17);

        assert_eq!(
            value(chain, metric::tx::TX_CONSECUTIVE_FAILURES),
            Point::F64(2.0)
        );
        assert_eq!(
            value(chain, metric::tx::TX_CHANNEL_UTILIZATION),
            Point::F64(17.0)
        );
    }

    #[test]
    fn packet_counters_skip_zero_counts() {
        let chain = "packet";
        let metrics = PacketMetrics::new(&label(chain)).with_counterparty(&ChainLabel::new("cp"));
        for count in [0, 2, 3] {
            metrics.record_recv(count);
            metrics.record_ack(count);
        }
        metrics.record_timeout(0);
        metrics.record_backlog(9);
        metrics.record_oldest_sequence(42);

        assert_eq!(value(chain, metric::packet::RECEIVE_PACKETS), Point::U64(5));
        assert_eq!(value(chain, metric::packet::ACK_PACKETS), Point::U64(5));
        assert_eq!(collect(chain, metric::packet::TIMEOUT_PACKETS), vec![]);
        assert_eq!(value(chain, metric::backlog::BACKLOG_SIZE), Point::F64(9.0));
        assert_eq!(
            value(chain, metric::backlog::BACKLOG_OLDEST_SEQUENCE),
            Point::F64(42.0)
        );

        metrics.record_timeout(1);
        assert_eq!(value(chain, metric::packet::TIMEOUT_PACKETS), Point::U64(1));
    }

    #[test]
    fn event_metrics_record_counts_and_modes() {
        let chain = "events";
        let metrics = EventMetrics::new(&label(chain)).with_counterparty(&ChainLabel::new("cp"));
        for count in [0, 4] {
            metrics.record_send_events(count);
            metrics.record_ack_events(count);
            metrics.record_filtered(count);
            metrics.record_ws_events(count);
        }
        metrics.record_event_source_mode(true);
        metrics.record_event_source_mode(false);
        metrics.record_ws_reconnect_attempt();
        metrics.record_ws_reconnect_attempt();
        metrics.record_ws_reconnect_failed();
        metrics.record_ws_fallback();
        metrics.record_ws_connected();
        metrics.record_lag(Instant::now());

        for name in [
            metric::event::SEND_PACKET_EVENTS,
            metric::event::ACK_EVENTS,
            metric::packet::FILTERED_PACKETS,
            metric::event::WS_EVENTS_RECEIVED_TOTAL,
        ] {
            assert_eq!(value(chain, name), Point::U64(4), "{name}");
        }
        assert_eq!(
            value(chain, metric::event::EVENT_SOURCE_MODE),
            Point::F64(0.0)
        );
        assert_eq!(
            value(chain, metric::event::WS_RECONNECT_TOTAL),
            Point::U64(2)
        );
        assert_eq!(
            value(chain, metric::event::WS_RECONNECT_FAILED_TOTAL),
            Point::U64(1)
        );
        assert_eq!(
            value(chain, metric::event::WS_FALLBACK_TOTAL),
            Point::U64(1)
        );
        assert!(matches!(
            value(chain, metric::event::WS_LAST_CONNECT_TIMESTAMP_SECONDS),
            Point::F64(ts) if ts > 1.0e9
        ));
        assert!(matches!(
            value(chain, metric::event::EVENT_WATCHER_LAG_SECS),
            Point::F64(lag) if lag >= 0.0
        ));
    }

    #[test]
    fn sweep_metrics_skip_zero_and_tag_error_phase() {
        let chain = "sweep";
        let metrics = SweepMetrics::new(&label(chain)).with_counterparty(&ChainLabel::new("cp"));
        for count in [0, 6] {
            metrics.record_swept(count);
            metrics.record_recv_cleared(count);
            metrics.record_ack_cleared(count);
            metrics.record_excluded(count);
        }
        metrics.record_scan_duration(Duration::from_millis(500));
        metrics.record_error("recv");
        metrics.record_error("ack");
        metrics.record_error("ack");

        for name in [
            metric::sweep::SWEPT_EVENTS,
            metric::sweep::SWEEP_RECV_CLEARED,
            metric::sweep::SWEEP_ACK_CLEARED,
            metric::sweep::SWEEP_EXCLUDED,
        ] {
            assert_eq!(value(chain, name), Point::U64(6), "{name}");
        }
        assert_eq!(
            value(chain, metric::sweep::SWEEP_SCAN_DURATION_SECONDS),
            Point::Hist { count: 1, sum: 0.5 }
        );
        let mut errors: Vec<(String, Point)> = collect(chain, metric::sweep::SWEEP_ERRORS)
            .into_iter()
            .map(|(attrs, p)| (attr(&attrs, "phase").unwrap(), p))
            .collect();
        errors.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            errors,
            vec![
                ("ack".to_owned(), Point::U64(2)),
                ("recv".to_owned(), Point::U64(1)),
            ]
        );
    }

    #[test]
    fn client_metrics_carry_client_id() {
        let chain = "client";
        let metrics = ClientMetrics::new(&label(chain))
            .with_counterparty(&ChainLabel::new("cp"))
            .with_client_id("07-tendermint-0");
        metrics.record_update_submitted();
        metrics.record_update_skipped();
        metrics.record_update_skipped();

        let (attrs, submitted) = single(chain, metric::client::CLIENT_UPDATES_SUBMITTED);
        assert_eq!(submitted, Point::U64(1));
        assert_eq!(
            attr(&attrs, "client_id").as_deref(),
            Some("07-tendermint-0")
        );
        assert_eq!(
            value(chain, metric::client::CLIENT_UPDATES_SKIPPED),
            Point::U64(2)
        );
    }

    #[test]
    fn misbehaviour_metrics_count_submissions() {
        let chain = "misbehaviour";
        let metrics = MisbehaviourMetrics::new(&label(chain))
            .with_counterparty(&ChainLabel::new("cp"))
            .with_client_id("client-9");
        metrics.record_submitted();

        let (attrs, point) = single(chain, metric::client::MISBEHAVIOURS_SUBMITTED);
        assert_eq!(point, Point::U64(1));
        assert_eq!(attr(&attrs, "client_id").as_deref(), Some("client-9"));
        assert_eq!(
            attr(&attrs, "counterparty_chain_name").as_deref(),
            Some("cp")
        );
    }

    #[test]
    fn wallet_balance_is_tagged_by_account_and_denom() {
        let chain = "wallet";
        let metrics = WalletMetrics::new(&label(chain));
        metrics.record_balance(12.5, "relayer", "uatom");

        let (attrs, point) = single(chain, metric::wallet::WALLET_BALANCE);
        assert_eq!(point, Point::F64(12.5));
        assert_eq!(attr(&attrs, "account").as_deref(), Some("relayer"));
        assert_eq!(attr(&attrs, "denom").as_deref(), Some("uatom"));
    }

    #[test]
    fn gas_price_histogram_records_samples() {
        let chain = "gas";
        let metrics = GasMetrics::new(&label(chain));
        metrics.record_gas_price(1.5);
        metrics.record_gas_price(2.5);

        assert_eq!(
            value(chain, metric::gas::GAS_PRICE_GWEI),
            Point::Hist { count: 2, sum: 4.0 }
        );
        assert_eq!(format!("{metrics:?}"), "GasMetrics { .. }");
    }

    #[test]
    fn chain_id_is_included_in_attributes() {
        let chain = "with-id";
        harness();
        let metrics = MisbehaviourMetrics::new(&ChainLabel::with_id(chain, "cosmoshub-4"));
        metrics.record_submitted();

        let (attrs, _) = single(chain, metric::client::MISBEHAVIOURS_SUBMITTED);
        assert_eq!(attr(&attrs, "chain_id").as_deref(), Some("cosmoshub-4"));
    }
}
