use mercury_chain_traits::types::{MessageSender, TxReceipt};
use mercury_core::error::{Result, eyre};
use tokio::sync::{mpsc, oneshot};
use tracing::debug;

const COORDINATOR_CHANNEL_BUFFER: usize = 256;

struct TxSubmission<M> {
    messages: Vec<M>,
    response: oneshot::Sender<Result<TxReceipt>>,
}

#[derive(Clone)]
pub struct TxCoordinatorHandle<M> {
    sender: mpsc::Sender<TxSubmission<M>>,
}

impl<M: Send + 'static> TxCoordinatorHandle<M> {
    pub async fn submit(&self, messages: Vec<M>) -> Result<TxReceipt> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(TxSubmission {
                messages,
                response: tx,
            })
            .await
            .map_err(|_| eyre!("tx coordinator closed"))?;
        rx.await
            .map_err(|_| eyre!("tx coordinator dropped response"))?
    }
}

/// Exits when all senders are dropped
pub fn spawn_coordinator<C>(chain: C) -> TxCoordinatorHandle<C::Message>
where
    C: MessageSender + Send + 'static,
    C::Message: Send + 'static,
{
    let (tx, rx) = mpsc::channel(COORDINATOR_CHANNEL_BUFFER);
    tokio::spawn(run_coordinator(chain, rx));
    TxCoordinatorHandle { sender: tx }
}

async fn run_coordinator<C>(chain: C, mut rx: mpsc::Receiver<TxSubmission<C::Message>>)
where
    C: MessageSender + Send + 'static,
    C::Message: Send + 'static,
{
    while let Some(first) = rx.recv().await {
        let mut all_messages = first.messages;
        let mut responses = vec![first.response];

        while let Ok(sub) = rx.try_recv() {
            all_messages.extend(sub.messages);
            responses.push(sub.response);
        }

        let caller_count = responses.len();
        if caller_count > 1 {
            debug!(
                msg_count = all_messages.len(),
                caller_count, "coalesced messages from multiple callers"
            );
        }

        // eyre::Report is not Clone, so convert the error to a shareable string
        // before broadcasting to all waiting callers.
        let broadcast: Result<TxReceipt, String> = chain
            .send_messages(all_messages)
            .await
            .map_err(|e| e.to_string());

        for response in responses {
            let result = broadcast.clone().map_err(|msg| eyre!("{msg}"));
            let _ = response.send(result);
        }
    }
}
