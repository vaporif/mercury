use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::ThreadSafe;

#[async_trait]
pub trait Runtime: ThreadSafe {
    type JoinHandle<T: Send + 'static>: Future<Output = std::result::Result<T, tokio::task::JoinError>>
        + Send
        + Unpin;

    async fn sleep(&self, duration: Duration);

    fn now(&self) -> Instant;

    fn spawn<F>(&self, name: &str, fut: F) -> Self::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static;
}

#[derive(Clone)]
pub struct TokioRuntime {
    pub runtime: Arc<tokio::runtime::Runtime>,
}

impl TokioRuntime {
    pub const fn new(runtime: Arc<tokio::runtime::Runtime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl Runtime for TokioRuntime {
    type JoinHandle<T: Send + 'static> = tokio::task::JoinHandle<T>;

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn spawn<F>(&self, name: &str, fut: F) -> Self::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let name = name.to_owned();
        self.runtime.spawn(async move {
            tracing::debug!(task = %name, "spawned");
            fut.await
        })
    }
}
