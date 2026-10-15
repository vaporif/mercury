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

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo {
        tx: tokio::sync::oneshot::Sender<&'static str>,
        fail: bool,
    }

    #[async_trait]
    impl Worker for Echo {
        fn name(&self) -> &'static str {
            "echo"
        }

        async fn run(self) -> Result<()> {
            let name = self.name();
            self.tx.send(name).ok();
            if self.fail {
                eyre::bail!("echo failed");
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn spawn_worker_runs_worker_to_completion() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let handle = spawn_worker(Echo { tx, fail: false });
        handle.await.unwrap().unwrap();
        assert_eq!(rx.await.unwrap(), "echo");
    }

    #[tokio::test]
    async fn spawn_worker_surfaces_worker_error_through_handle() {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let err = spawn_worker(Echo { tx, fail: true })
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err.to_string(), "echo failed");
    }
}
