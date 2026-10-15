use opentelemetry::metrics::UpDownCounter;
use opentelemetry::{KeyValue, global};

use mercury_core::ChainLabel;

use crate::metric;

/// RAII guard that increments the worker gauge on creation and decrements on drop.
pub struct WorkerGuard {
    attrs: Vec<KeyValue>,
    counter: UpDownCounter<i64>,
}

impl WorkerGuard {
    #[must_use]
    pub fn new(worker_type: &'static str) -> Self {
        let attrs = vec![KeyValue::new("type", worker_type)];
        let counter = global::meter("mercury_telemetry")
            .i64_up_down_counter(metric::worker::WORKERS)
            .build();
        counter.add(1, &attrs);
        Self { attrs, counter }
    }

    #[must_use]
    pub fn with_chain_labels(
        worker_type: &'static str,
        chain: &ChainLabel,
        counterparty: Option<&ChainLabel>,
    ) -> Self {
        let mut attrs = vec![KeyValue::new("type", worker_type)];
        for (k, v) in chain.metric_labels() {
            attrs.push(KeyValue::new(k, v));
        }
        if let Some(cp) = counterparty {
            for (k, v) in cp.counterparty_metric_labels() {
                attrs.push(KeyValue::new(k, v));
            }
        }
        let counter = global::meter("mercury_telemetry")
            .i64_up_down_counter(metric::worker::WORKERS)
            .build();
        counter.add(1, &attrs);
        Self { attrs, counter }
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.counter.add(-1, &self.attrs);
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ScopeMetrics};

    use super::*;
    use crate::recorder::tests::harness;

    // Current `workers` value for the series whose `type` attribute is `worker_type`.
    fn workers(worker_type: &str) -> Option<(i64, Vec<KeyValue>)> {
        let (provider, exporter) = harness();
        provider.force_flush().unwrap();
        let exports = exporter.get_finished_metrics().unwrap();
        let rm = exports.last()?;
        rm.scope_metrics()
            .flat_map(ScopeMetrics::metrics)
            .filter(|m| m.name() == metric::worker::WORKERS)
            .find_map(|m| match m.data() {
                AggregatedMetrics::I64(MetricData::Sum(sum)) => sum
                    .data_points()
                    .find(|dp| {
                        dp.attributes()
                            .any(|kv| kv.key.as_str() == "type" && kv.value.as_str() == worker_type)
                    })
                    .map(|dp| (dp.value(), dp.attributes().cloned().collect())),
                _ => None,
            })
    }

    fn attr(attrs: &[KeyValue], key: &str) -> Option<String> {
        attrs
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.as_str().into_owned())
    }

    #[test]
    fn worker_gauge_counts_live_guards() {
        harness();
        let first = WorkerGuard::new("guard-test-live");
        let second = WorkerGuard::new("guard-test-live");
        assert_eq!(workers("guard-test-live").unwrap().0, 2);
        drop(first);
        assert_eq!(workers("guard-test-live").unwrap().0, 1);
        drop(second);
        assert_eq!(workers("guard-test-live").unwrap().0, 0);
    }

    #[test]
    fn worker_guard_carries_chain_and_counterparty_labels() {
        harness();
        let chain = ChainLabel::with_id("cosmos", "cosmoshub-4");
        let cp = ChainLabel::new("ethereum");
        let guard = WorkerGuard::with_chain_labels("guard-test-labels", &chain, Some(&cp));
        let (value, attrs) = workers("guard-test-labels").unwrap();
        assert_eq!(value, 1);
        assert_eq!(attr(&attrs, "chain_name").as_deref(), Some("cosmos"));
        assert_eq!(attr(&attrs, "chain_id").as_deref(), Some("cosmoshub-4"));
        assert_eq!(
            attr(&attrs, "counterparty_chain_name").as_deref(),
            Some("ethereum")
        );
        assert_eq!(attr(&attrs, "counterparty_chain_id"), None);
        drop(guard);
        assert_eq!(workers("guard-test-labels").unwrap().0, 0);
    }

    #[test]
    fn worker_guard_without_counterparty_has_no_counterparty_labels() {
        harness();
        let chain = ChainLabel::new("solana");
        let _guard = WorkerGuard::with_chain_labels("guard-test-solo", &chain, None);
        let (value, attrs) = workers("guard-test-solo").unwrap();
        assert_eq!(value, 1);
        assert_eq!(attr(&attrs, "chain_name").as_deref(), Some("solana"));
        assert!(
            attrs
                .iter()
                .all(|kv| !kv.key.as_str().starts_with("counterparty_"))
        );
    }
}
