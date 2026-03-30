use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use mercury_chain_traits::builders::{ClientMessageBuilder, ClientPayloadBuilder};
use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery};
use mercury_chain_traits::relay::Relay;
use mercury_chain_traits::types::{ChainTypes, IbcTypes};
use mercury_core::error::Result;
use mercury_core::worker::Worker;

use mercury_telemetry::recorder::ClientMetrics;

use crate::workers::DstTxRequest;

const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

/// Keeps the dst client alive by refreshing it before it expires.
pub struct ClientRefreshWorker<R: Relay> {
    pub relay: Arc<R>,
    pub sender: mpsc::Sender<DstTxRequest<R>>,
    pub token: CancellationToken,
    pub metrics: ClientMetrics,
    pub last_upgrade_plan_height: Option<i64>,
}

impl<R: Relay> ClientRefreshWorker<R> {
    async fn query_dst_client_state(
        &self,
    ) -> eyre::Result<(
        <R::DstChain as ChainTypes>::Height,
        <R::DstChain as IbcTypes>::ClientState,
    )> {
        type DstChain<R> = <R as Relay>::DstChain;

        let dst_status = self.relay.dst_chain().query_chain_status().await?;
        let dst_height = DstChain::<R>::chain_status_height(&dst_status).clone();
        let cs = self
            .relay
            .dst_chain()
            .query_client_state(self.relay.dst_client_id(), &dst_height)
            .await?;
        Ok((dst_height, cs))
    }

    async fn try_submit_upgrade(&mut self) -> Result<()> {
        let Some(payload) = self
            .relay
            .src_chain()
            .build_upgrade_client_payload()
            .await?
        else {
            return Ok(());
        };

        if self.last_upgrade_plan_height == Some(payload.plan_height) {
            return Ok(());
        }

        info!("source chain upgrade detected, submitting client upgrade");
        let plan_height = payload.plan_height;
        let messages = self
            .relay
            .dst_chain()
            .build_upgrade_client_message(self.relay.dst_client_id(), payload)
            .await?;

        if messages.is_empty() {
            return Ok(());
        }

        self.sender
            .send(DstTxRequest {
                messages,
                created_at: std::time::Instant::now(),
            })
            .await
            .map_err(|_| eyre::eyre!("tx_worker channel closed during upgrade"))?;

        self.last_upgrade_plan_height = Some(plan_height);
        info!("client upgrade message submitted");
        Ok(())
    }
}

#[async_trait]
impl<R: Relay> Worker for ClientRefreshWorker<R> {
    fn name(&self) -> &'static str {
        "client_refresh"
    }

    #[instrument(skip_all, name = "client_refresh", fields(src_chain = %self.relay.src_chain().chain_label(), dst_chain = %self.relay.dst_chain().chain_label()))]
    async fn run(mut self) -> Result<()> {
        type SrcChain<R> = <R as Relay>::SrcChain;
        type DstChain<R> = <R as Relay>::DstChain;

        let mut check_interval = DEFAULT_REFRESH_INTERVAL;

        loop {
            tokio::select! {
                () = self.token.cancelled() => break,
                () = tokio::time::sleep(check_interval) => {}
            }

            let Ok((_dst_height, client_state)) = self
                .query_dst_client_state()
                .await
                .inspect_err(|e| warn!(error = %e, "client refresh: failed to query chain/client state, will retry"))
            else {
                continue;
            };

            check_interval = DstChain::<R>::trusting_period(&client_state)
                .map_or(DEFAULT_REFRESH_INTERVAL, |tp| tp / 3);
            debug!(
                interval_secs = check_interval.as_secs(),
                "next client refresh check"
            );

            let current_trusted = DstChain::<R>::client_latest_height(&client_state);

            let Ok(target_status) = self
                .relay
                .src_chain()
                .query_chain_status()
                .await
                .inspect_err(|e| warn!(error = %e, "client refresh: failed to query src chain status, will retry"))
            else {
                continue;
            };
            let target_height = SrcChain::<R>::chain_status_height(&target_status).clone();

            if target_height <= current_trusted {
                debug!("client already up to date, skipping refresh");
                self.metrics.record_update_skipped();
                continue;
            }

            match async {
                let payload = self
                    .relay
                    .src_chain()
                    .build_update_client_payload(&current_trusted, &target_height, &client_state)
                    .await?;

                if let Some(required_ts) =
                    self.relay.src_chain().required_dst_timestamp_secs(&payload)
                {
                    super::wait_for_dst_timestamp::<R>(self.relay.dst_chain(), required_ts).await?;
                }

                self.relay
                    .dst_chain()
                    .build_update_client_message(self.relay.dst_client_id(), payload)
                    .await
            }
            .await
            {
                Ok(output) => {
                    let messages = output.messages;
                    self.metrics.record_update_submitted();
                    info!("refreshing client");
                    if self
                        .sender
                        .send(DstTxRequest {
                            messages,
                            created_at: std::time::Instant::now(),
                        })
                        .await
                        .is_err()
                    {
                        warn!("tx_worker channel closed");
                        break;
                    }

                    if let Err(e) = self.try_submit_upgrade().await {
                        debug!(error = %e, "upgrade check skipped");
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
