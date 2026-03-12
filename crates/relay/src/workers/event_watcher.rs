use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mercury_chain_traits::relay::context::Relay;
use mercury_chain_traits::relay::ibc_event::IbcEvent;
use mercury_core::error::Result;
use mercury_core::worker::Worker;

pub struct EventWatcher<R: Relay> {
    pub relay: Arc<R>,
    pub sender: mpsc::Sender<Vec<IbcEvent<R>>>,
    pub token: CancellationToken,
}

#[async_trait]
impl<R: Relay> Worker for EventWatcher<R> {
    fn name(&self) -> &'static str {
        "event_watcher"
    }

    async fn run(self) -> Result<()> {
        todo!("EventWatcher::run")
    }
}
