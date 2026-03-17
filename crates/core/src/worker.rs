use async_trait::async_trait;
use tokio::task::JoinHandle;

use crate::error::Result;

#[async_trait]
pub trait Worker: Send + 'static {
    fn name(&self) -> &'static str;
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
