use metrics::gauge;

use mercury_core::ChainLabel;

use crate::metric;

/// RAII guard that increments the worker gauge on creation and decrements on drop.
pub struct WorkerGuard {
    labels: Vec<(&'static str, String)>,
}

impl WorkerGuard {
    #[must_use]
    pub fn new(worker_type: &'static str) -> Self {
        let labels = vec![("type", worker_type.to_owned())];
        gauge!(metric::worker::WORKERS, &labels).increment(1.0);
        Self { labels }
    }

    #[must_use]
    pub fn with_chain_labels(
        worker_type: &'static str,
        chain: &ChainLabel,
        counterparty: Option<&ChainLabel>,
    ) -> Self {
        let mut labels = vec![("type", worker_type.to_owned())];
        labels.extend(chain.metric_labels());
        if let Some(cp) = counterparty {
            labels.extend(cp.counterparty_metric_labels());
        }
        gauge!(metric::worker::WORKERS, &labels).increment(1.0);
        Self { labels }
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        gauge!(metric::worker::WORKERS, &self.labels).decrement(1.0);
    }
}
