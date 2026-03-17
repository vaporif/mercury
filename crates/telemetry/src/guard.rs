use metrics::gauge;

use crate::metric;

/// RAII guard that increments the worker gauge on creation and decrements on drop.
pub struct WorkerGuard {
    worker_type: &'static str,
}

impl WorkerGuard {
    #[must_use]
    pub fn new(worker_type: &'static str) -> Self {
        gauge!(metric::worker::WORKERS, "type" => worker_type).increment(1.0);
        Self { worker_type }
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        gauge!(metric::worker::WORKERS, "type" => self.worker_type).decrement(1.0);
    }
}
