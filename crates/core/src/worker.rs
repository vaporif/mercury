//! Async worker abstraction and spawning helpers.

use async_trait::async_trait;
use tokio::task::JoinHandle;

use crate::error::Result;

/// A named async task that can be spawned onto the Tokio runtime.
#[async_trait]
pub trait Worker: Send + 'static {
    /// Returns the worker's name, used for logging.
    fn name(&self) -> &'static str;
    /// Run the worker to completion.
    async fn run(self) -> Result<()>;
}

/// Spawn a [`Worker`] as a Tokio task, returning its join handle.
#[must_use]
pub fn spawn_worker<W: Worker>(worker: W) -> JoinHandle<Result<()>> {
    let name = worker.name().to_owned();
    tokio::spawn(async move {
        tracing::debug!(task = %name, "spawned");
        worker.run().await
    })
}
