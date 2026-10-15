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

const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_mins(5);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testhelpers::*;

    const FIRST_CHECK: Duration = DEFAULT_REFRESH_INTERVAL;

    fn worker(
        src: MockState,
        dst: MockState,
    ) -> (
        ClientRefreshWorker<MockRelay>,
        mpsc::Receiver<DstTxRequest<MockRelay>>,
        SharedState,
        SharedState,
    ) {
        let (relay, src_state, dst_state) = mock_relay(src, dst);
        let (sender, rx) = mpsc::channel(8);
        let worker = ClientRefreshWorker {
            relay,
            sender,
            token: CancellationToken::new(),
            metrics: ClientMetrics::new(&mock_label()),
            last_upgrade_plan_height: None,
        };
        (worker, rx, src_state, dst_state)
    }

    fn src_at(height: u64) -> MockState {
        MockState {
            latest_height: height,
            ..Default::default()
        }
    }

    fn dst_with_update() -> MockState {
        MockState {
            update_messages: 1,
            ..Default::default()
        }
    }

    /// Runs the worker for `duration` of paused time, then cancels it.
    async fn run_for(
        worker: ClientRefreshWorker<MockRelay>,
        duration: Duration,
    ) -> CancellationToken {
        let token = worker.token.clone();
        let handle = tokio::spawn(worker.run());
        tokio::time::sleep(duration).await;
        token.cancel();
        handle.await.unwrap().unwrap();
        token
    }

    #[tokio::test(start_paused = true)]
    async fn refreshes_lagging_client_then_rechecks_at_third_of_trusting_period() {
        let (worker, mut rx, _, _) = worker(src_at(150), dst_with_update());
        let token = worker.token.clone();
        let handle = tokio::spawn(worker.run());
        let start = tokio::time::Instant::now();

        let req = recv(&mut rx).await;
        assert_eq!(req.messages.len(), 1);
        assert_eq!(start.elapsed(), FIRST_CHECK);

        // Default mock trusting period is 1h, so the next check is 20 min later.
        recv(&mut rx).await;
        assert_eq!(start.elapsed(), FIRST_CHECK + Duration::from_mins(20));

        token.cancel();
        handle.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn keeps_default_interval_without_trusting_period() {
        let dst = MockState {
            trusting_period: None,
            ..dst_with_update()
        };
        let (worker, mut rx, _, _) = worker(src_at(150), dst);
        let token = worker.token.clone();
        let handle = tokio::spawn(worker.run());
        let start = tokio::time::Instant::now();

        recv(&mut rx).await;
        recv(&mut rx).await;
        assert_eq!(start.elapsed(), 2 * FIRST_CHECK);

        token.cancel();
        handle.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn skips_refresh_when_client_up_to_date() {
        let (worker, mut rx, src_state, _) = worker(src_at(100), dst_with_update());
        run_for(worker, FIRST_CHECK + Duration::from_secs(1)).await;

        assert!(rx.try_recv().is_err());
        assert_eq!(src_state.lock().unwrap().update_payload_calls, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn dst_query_failure_retries_without_sending() {
        let dst = MockState {
            status_error: true,
            ..dst_with_update()
        };
        let (worker, mut rx, src_state, dst_state) = worker(src_at(150), dst);
        run_for(worker, 2 * FIRST_CHECK + Duration::from_secs(1)).await;

        assert!(rx.try_recv().is_err());
        assert_eq!(dst_state.lock().unwrap().status_queries, 2);
        assert_eq!(src_state.lock().unwrap().status_queries, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn src_query_failure_retries_without_sending() {
        let src = MockState {
            status_error: true,
            ..src_at(150)
        };
        let (worker, mut rx, src_state, _) = worker(src, dst_with_update());
        run_for(worker, FIRST_CHECK + Duration::from_secs(1)).await;

        assert!(rx.try_recv().is_err());
        assert_eq!(src_state.lock().unwrap().update_payload_calls, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn payload_build_failure_retries_next_interval() {
        let src = MockState {
            update_payload_error: true,
            ..src_at(150)
        };
        let (worker, mut rx, src_state, _) = worker(src, dst_with_update());
        run_for(worker, FIRST_CHECK + Duration::from_secs(1201)).await;

        assert!(rx.try_recv().is_err());
        assert_eq!(src_state.lock().unwrap().update_payload_calls, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn waits_for_dst_timestamp_before_submitting() {
        let src = MockState {
            required_dst_timestamp: Some(50),
            ..src_at(150)
        };
        let (worker, mut rx, _, dst_state) = worker(src, dst_with_update());
        let token = worker.token.clone();
        let handle = tokio::spawn(worker.run());
        let start = tokio::time::Instant::now();

        tokio::time::sleep(FIRST_CHECK + Duration::from_secs(12)).await;
        assert!(
            rx.try_recv().is_err(),
            "must not submit before dst catches up"
        );
        dst_state.lock().unwrap().timestamp_secs = 50;

        recv(&mut rx).await;
        // Polled at +0s, +5s, +10s; the +15s poll sees the new timestamp.
        assert_eq!(start.elapsed(), FIRST_CHECK + Duration::from_secs(15));

        token.cancel();
        handle.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn gives_up_when_dst_timestamp_never_catches_up() {
        let src = MockState {
            required_dst_timestamp: Some(50),
            ..src_at(150)
        };
        let (worker, mut rx, _, _) = worker(src, dst_with_update());
        run_for(worker, FIRST_CHECK + Duration::from_mins(16)).await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn exits_when_tx_channel_closed() {
        let (worker, rx, _, _) = worker(src_at(150), dst_with_update());
        drop(rx);
        let token = worker.token.clone();
        worker.run().await.unwrap();
        assert!(!token.is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn submits_upgrade_after_refresh() {
        let src = MockState {
            upgrade_plan_height: Some(500),
            ..src_at(150)
        };
        let dst = MockState {
            upgrade_messages: 2,
            ..dst_with_update()
        };
        let (worker, mut rx, _, _) = worker(src, dst);
        run_for(worker, FIRST_CHECK + Duration::from_secs(1201)).await;

        let sizes: Vec<usize> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|r| r.messages.len())
            .collect();
        // update, upgrade, then only the update on the second pass.
        assert_eq!(sizes, vec![1, 2, 1]);
    }

    #[tokio::test]
    async fn upgrade_noop_without_plan() {
        let (mut worker, mut rx, _, _) = worker(MockState::default(), MockState::default());
        worker.try_submit_upgrade().await.unwrap();
        assert!(rx.try_recv().is_err());
        assert_eq!(worker.last_upgrade_plan_height, None);
    }

    #[tokio::test]
    async fn upgrade_with_no_messages_is_not_recorded() {
        let src = MockState {
            upgrade_plan_height: Some(7),
            ..Default::default()
        };
        let (mut worker, mut rx, _, _) = worker(src, MockState::default());
        worker.try_submit_upgrade().await.unwrap();
        assert!(rx.try_recv().is_err());
        assert_eq!(worker.last_upgrade_plan_height, None);
    }

    #[tokio::test]
    async fn upgrade_submitted_once_per_plan_height() {
        let src = MockState {
            upgrade_plan_height: Some(7),
            ..Default::default()
        };
        let dst = MockState {
            upgrade_messages: 1,
            ..Default::default()
        };
        let (mut worker, mut rx, src_state, _) = worker(src, dst);

        worker.try_submit_upgrade().await.unwrap();
        worker.try_submit_upgrade().await.unwrap();
        assert_eq!(worker.last_upgrade_plan_height, Some(7));
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());

        src_state.lock().unwrap().upgrade_plan_height = Some(8);
        worker.try_submit_upgrade().await.unwrap();
        assert_eq!(worker.last_upgrade_plan_height, Some(8));
        assert!(rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn upgrade_errors_when_channel_closed() {
        let src = MockState {
            upgrade_plan_height: Some(7),
            ..Default::default()
        };
        let dst = MockState {
            upgrade_messages: 1,
            ..Default::default()
        };
        let (mut worker, rx, _, _) = worker(src, dst);
        drop(rx);
        assert!(worker.try_submit_upgrade().await.is_err());
        assert_eq!(worker.last_upgrade_plan_height, None);
    }
}
