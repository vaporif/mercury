use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use mercury_chain_traits::message_builders::CanBuildUpdateClientMessage;
use mercury_chain_traits::payload_builders::CanBuildUpdateClientPayload;
use mercury_chain_traits::queries::{
    CanQueryChainStatus, CanQueryClientState, HasClientLatestHeight, HasTrustingPeriod,
};
use mercury_chain_traits::relay::context::Relay;
use mercury_chain_traits::types::HasChainStatusType;
use mercury_core::error::Result;
use mercury_core::worker::Worker;

use crate::workers::TxRequest;

const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

pub struct ClientRefreshWorker<R: Relay> {
    pub relay: Arc<R>,
    pub sender: mpsc::Sender<TxRequest<R>>,
    pub token: CancellationToken,
}

#[async_trait]
impl<R> Worker for ClientRefreshWorker<R>
where
    R: Relay,
    R::SrcChain: CanBuildUpdateClientPayload<R::DstChain>,
    R::DstChain: CanQueryClientState<R::SrcChain>
        + HasClientLatestHeight<R::SrcChain>
        + HasTrustingPeriod<R::SrcChain>
        + CanBuildUpdateClientMessage<R::SrcChain>,
{
    fn name(&self) -> &'static str {
        "client_refresh"
    }

    async fn run(self) -> Result<()> {
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        loop {
            // Query client state to determine check interval
            let dst_status = self.relay.dst_chain().query_chain_status().await?;
            let dst_height = DstChain::<R>::chain_status_height(&dst_status).clone();

            let client_state = self
                .relay
                .dst_chain()
                .query_client_state(self.relay.dst_client_id(), &dst_height)
                .await?;

            let check_interval = DstChain::<R>::trusting_period(&client_state)
                .map_or(DEFAULT_REFRESH_INTERVAL, |tp| tp / 3);

            // Sleep (cancellation-aware)
            tokio::select! {
                () = self.token.cancelled() => break,
                () = tokio::time::sleep(check_interval) => {}
            }

            // Re-query to see if client was updated by PacketWorker
            let dst_status = self.relay.dst_chain().query_chain_status().await?;
            let dst_height = DstChain::<R>::chain_status_height(&dst_status).clone();
            let client_state = self
                .relay
                .dst_chain()
                .query_client_state(self.relay.dst_client_id(), &dst_height)
                .await?;
            let current_trusted = DstChain::<R>::client_latest_height(&client_state);

            // Get src chain latest height
            let src_status = self.relay.src_chain().query_chain_status().await?;
            let target_height = SrcChain::<R>::chain_status_height(&src_status).clone();

            if target_height <= current_trusted {
                debug!("client already up to date, skipping refresh");
                continue;
            }

            // Build and send MsgUpdateClient
            let payload = self
                .relay
                .src_chain()
                .build_update_client_payload(&current_trusted, &target_height)
                .await?;
            let messages = self
                .relay
                .dst_chain()
                .build_update_client_message(self.relay.dst_client_id(), payload)
                .await?;

            info!("refreshing client");
            if self.sender.send(TxRequest { messages }).await.is_err() {
                warn!("tx_worker channel closed");
                break;
            }
        }

        Ok(())
    }
}
