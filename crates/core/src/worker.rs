use async_trait::async_trait;

use crate::error::Result;
use crate::runtime::Runtime;

#[async_trait]
pub trait Worker: Send + 'static {
    fn name(&self) -> &'static str;
    async fn run(self) -> Result<()>;
}

pub fn spawn_worker<R, W>(runtime: &R, worker: W) -> R::JoinHandle<Result<()>>
where
    R: Runtime,
    W: Worker,
{
    let name = worker.name().to_owned();
    runtime.spawn(&name, async move { worker.run().await })
}
