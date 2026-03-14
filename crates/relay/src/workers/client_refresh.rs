use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use mercury_chain_traits::prelude::*;
use mercury_chain_traits::relay::Relay;
use mercury_core::error::Result;
use mercury_core::worker::Worker;

use crate::workers::DstTxRequest;

const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

/// Periodically refreshes the destination client to prevent expiry.
pub struct ClientRefreshWorker<R: Relay> {
    pub relay: Arc<R>,
    pub sender: mpsc::Sender<DstTxRequest<R>>,
    pub token: CancellationToken,
}

#[async_trait]
impl<R: Relay> Worker for ClientRefreshWorker<R> {
    fn name(&self) -> &'static str {
        "client_refresh"
    }

    #[instrument(skip_all, name = "client_refresh")]
    async fn run(self) -> Result<()> {
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        let mut check_interval = DEFAULT_REFRESH_INTERVAL;

        loop {
            // Sleep (cancellation-aware)
            tokio::select! {
                () = self.token.cancelled() => break,
                () = tokio::time::sleep(check_interval) => {}
            }

            // Query client state to determine next check interval
            let (_dst_height, client_state) = match async {
                let dst_status = self.relay.dst_chain().query_chain_status().await?;
                let dst_height = DstChain::<R>::chain_status_height(&dst_status).clone();
                let cs = self
                    .relay
                    .dst_chain()
                    .query_client_state(self.relay.dst_client_id(), &dst_height)
                    .await?;
                Ok::<_, eyre::Report>((dst_height, cs))
            }
            .await
            {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, "client refresh: failed to query chain/client state, will retry");
                    continue;
                }
            };

            check_interval = DstChain::<R>::trusting_period(&client_state)
                .map_or(DEFAULT_REFRESH_INTERVAL, |tp| tp / 3);

            let current_trusted = DstChain::<R>::client_latest_height(&client_state);

            // Get src chain latest height
            let target_height = match self.relay.src_chain().query_chain_status().await {
                Ok(status) => SrcChain::<R>::chain_status_height(&status).clone(),
                Err(e) => {
                    warn!(error = %e, "client refresh: failed to query src chain status, will retry");
                    continue;
                }
            };

            if target_height <= current_trusted {
                debug!("client already up to date, skipping refresh");
                continue;
            }

            // Build and send MsgUpdateClient
            match async {
                let payload = self
                    .relay
                    .src_chain()
                    .build_update_client_payload(&current_trusted, &target_height, &client_state)
                    .await?;
                self.relay
                    .dst_chain()
                    .build_update_client_message(self.relay.dst_client_id(), payload)
                    .await
            }
            .await
            {
                Ok(messages) => {
                    info!("refreshing client");
                    if self.sender.send(DstTxRequest { messages }).await.is_err() {
                        warn!("tx_worker channel closed");
                        break;
                    }
                }
                Err(e) => {
                    warn!(error = %e, "client refresh: failed to build update client messages, will retry");
                }
            }
        }

        Ok(())
    }
}
