use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mercury_chain_traits::relay::context::Relay;
use mercury_core::error::Result;
use mercury_core::worker::Worker;

use crate::workers::TxRequest;

pub struct TxWorker<R: Relay> {
    pub relay: Arc<R>,
    pub receiver: mpsc::Receiver<TxRequest<R>>,
    pub token: CancellationToken,
}

#[async_trait]
impl<R: Relay> Worker for TxWorker<R> {
    fn name(&self) -> &'static str {
        "tx_worker"
    }

    async fn run(self) -> Result<()> {
        todo!("TxWorker::run")
    }
}
