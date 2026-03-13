use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{error, instrument, warn};

use mercury_chain_traits::prelude::*;
use mercury_chain_traits::relay::Relay;
use mercury_core::error::Result;
use mercury_core::worker::Worker;

/// Monitors for light client misbehaviour by checking update headers against the source chain.
pub struct MisbehaviourWorker<R: Relay> {
    pub relay: Arc<R>,
    pub token: CancellationToken,
    pub scan_interval: Duration,
}

#[async_trait]
impl<R> Worker for MisbehaviourWorker<R>
where
    R: Relay,
    R::SrcChain: MisbehaviourDetector<R::DstChain>,
    R::DstChain: MisbehaviourQuery<R::SrcChain>
        + MisbehaviourMessageBuilder<R::SrcChain>
        + ClientQuery<R::SrcChain>,
{
    fn name(&self) -> &'static str {
        "misbehaviour_worker"
    }

    #[instrument(skip_all, name = "misbehaviour_worker")]
    async fn run(self) -> Result<()> {
        let mut last_scanned_height: Option<<R::SrcChain as ChainTypes>::Height> = None;

        loop {
            match self.scan(&mut last_scanned_height).await {
                Ok(true) => {
                    self.token.cancel();
                    return Ok(());
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(error = %e, "misbehaviour scan failed, will retry next interval");
                }
            }

            tokio::select! {
                () = self.token.cancelled() => break,
                () = tokio::time::sleep(self.scan_interval) => {}
            }
        }

        Ok(())
    }
}

impl<R> MisbehaviourWorker<R>
where
    R: Relay,
    R::SrcChain: MisbehaviourDetector<R::DstChain>,
    R::DstChain: MisbehaviourQuery<R::SrcChain>
        + MisbehaviourMessageBuilder<R::SrcChain>
        + ClientQuery<R::SrcChain>,
{
    /// Scan for misbehaviour. Returns true if misbehaviour was found and submitted.
    async fn scan(
        &self,
        last_scanned_height: &mut Option<<R::SrcChain as ChainTypes>::Height>,
    ) -> Result<bool> {
        let src = self.relay.src_chain();
        let dst = self.relay.dst_chain();
        let dst_client_id = self.relay.dst_client_id();

        let dst_height = dst.query_latest_height().await?;
        let client_state = dst.query_client_state(dst_client_id, &dst_height).await?;

        let heights = dst.query_consensus_state_heights(dst_client_id).await?;

        if heights.is_empty() {
            return Ok(false);
        }

        let heights_to_check: Vec<_> = if let Some(last) = last_scanned_height {
            heights.into_iter().filter(|h| h > last).collect()
        } else {
            heights
        };

        if heights_to_check.is_empty() {
            return Ok(false);
        }

        for height in &heights_to_check {
            let header = match dst.query_update_client_header(dst_client_id, height).await {
                Ok(Some(h)) => h,
                Ok(None) => {
                    warn!(
                        height = %height,
                        "update_client event pruned from tx index, skipping"
                    );
                    continue;
                }
                Err(e) => {
                    warn!(
                        height = %height,
                        error = %e,
                        "failed to query update header, skipping"
                    );
                    continue;
                }
            };

            match src
                .check_for_misbehaviour(dst_client_id, &header, &client_state)
                .await
            {
                Ok(Some(evidence)) => {
                    error!(
                        height = %height,
                        "MISBEHAVIOUR EVIDENCE FOUND — submitting to chain"
                    );

                    let msg = dst
                        .build_misbehaviour_message(dst_client_id, evidence)
                        .await?;

                    dst.send_messages(vec![msg]).await?;

                    error!("Misbehaviour evidence submitted — shutting down relay pair");
                    return Ok(true);
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(
                        height = %height,
                        error = %e,
                        "misbehaviour check failed for height, skipping"
                    );
                }
            }
        }

        if let Some(max_height) = heights_to_check.into_iter().max() {
            *last_scanned_height = Some(max_height);
        }

        Ok(false)
    }
}
