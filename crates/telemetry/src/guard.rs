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
