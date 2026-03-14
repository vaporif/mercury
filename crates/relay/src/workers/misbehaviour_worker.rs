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
    R::SrcChain: MisbehaviourDetector<R::DstChain,
        CounterpartyClientState = <R::DstChain as IbcTypes>::ClientState,
    >,
    R::DstChain: MisbehaviourQuery<R::SrcChain,
        CounterpartyUpdateHeader = <R::SrcChain as MisbehaviourDetector<R::DstChain>>::UpdateHeader,
    > + MisbehaviourMessageBuilder<R::SrcChain,
        MisbehaviourEvidence = <R::SrcChain as MisbehaviourDetector<R::DstChain>>::MisbehaviourEvidence,
    >,
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
    R::SrcChain: MisbehaviourDetector<R::DstChain,
        CounterpartyClientState = <R::DstChain as IbcTypes>::ClientState,
    >,
    R::DstChain: MisbehaviourQuery<R::SrcChain,
        CounterpartyUpdateHeader = <R::SrcChain as MisbehaviourDetector<R::DstChain>>::UpdateHeader,
    > + MisbehaviourMessageBuilder<R::SrcChain,
        MisbehaviourEvidence = <R::SrcChain as MisbehaviourDetector<R::DstChain>>::MisbehaviourEvidence,
    >,
{
    /// Scan for misbehaviour. Returns `true` if misbehaviour was found and submitted.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use crate::testhelpers::*;

    fn make_worker(
        src_state: MockState,
        dst_state: MockState,
    ) -> (
        MisbehaviourWorker<MockRelay>,
        Arc<Mutex<MockState>>,
        Arc<Mutex<MockState>>,
    ) {
        let src_state = Arc::new(Mutex::new(src_state));
        let dst_state = Arc::new(Mutex::new(dst_state));

        let relay = MockRelay {
            src: MockChain::new(Arc::clone(&src_state)),
            dst: MockChain::new(Arc::clone(&dst_state)),
            src_client_id: "src-client".into(),
            dst_client_id: "dst-client".into(),
        };

        let worker = MisbehaviourWorker {
            relay: Arc::new(relay),
            token: CancellationToken::new(),
            scan_interval: Duration::from_millis(10),
        };

        (worker, src_state, dst_state)
    }

    #[tokio::test]
    async fn scan_no_heights_returns_false() {
        let (worker, _, _) = make_worker(MockState::default(), MockState::default());
        let mut last = None;
        assert!(!worker.scan(&mut last).await.unwrap());
        assert!(last.is_none());
    }

    #[tokio::test]
    async fn scan_no_new_heights_after_last_scanned() {
        let dst_state = MockState {
            consensus_heights: vec![5, 3, 1],
            ..Default::default()
        };
        let (worker, _, _) = make_worker(MockState::default(), dst_state);
        let mut last = Some(10u64);
        assert!(!worker.scan(&mut last).await.unwrap());
        assert_eq!(last, Some(10));
    }

    #[tokio::test]
    async fn scan_no_misbehaviour_found() {
        let dst_state = MockState {
            consensus_heights: vec![10, 5, 3],
            ..Default::default()
        };
        let (worker, _, _) = make_worker(MockState::default(), dst_state);
        let mut last = None;
        assert!(!worker.scan(&mut last).await.unwrap());
        assert_eq!(last, Some(10));
    }

    #[tokio::test]
    async fn scan_misbehaviour_detected_submits_and_returns_true() {
        let src_state = MockState {
            check_results: HashMap::from([(5, CheckResult::Evidence)]),
            ..Default::default()
        };
        let dst_state = MockState {
            consensus_heights: vec![10, 5, 3],
            ..Default::default()
        };
        let (worker, _, dst_arc) = make_worker(src_state, dst_state);
        let mut last = None;
        assert!(worker.scan(&mut last).await.unwrap());
        let dst = dst_arc.lock().unwrap();
        assert_eq!(dst.messages_sent.len(), 1);
        assert!(last.is_none());
    }

    #[tokio::test]
    async fn scan_pruned_header_skipped() {
        let dst_state = MockState {
            consensus_heights: vec![10, 5],
            headers: HashMap::from([(5, HeaderResult::Pruned)]),
            ..Default::default()
        };
        let (worker, _, _) = make_worker(MockState::default(), dst_state);
        let mut last = None;
        assert!(!worker.scan(&mut last).await.unwrap());
        assert_eq!(last, Some(10));
    }

    #[tokio::test]
    async fn scan_header_query_error_skipped() {
        let dst_state = MockState {
            consensus_heights: vec![7],
            headers: HashMap::from([(7, HeaderResult::Err)]),
            ..Default::default()
        };
        let (worker, _, _) = make_worker(MockState::default(), dst_state);
        let mut last = None;
        assert!(!worker.scan(&mut last).await.unwrap());
        assert_eq!(last, Some(7));
    }

    #[tokio::test]
    async fn scan_check_error_skipped() {
        let src_state = MockState {
            check_results: HashMap::from([(5, CheckResult::Err)]),
            ..Default::default()
        };
        let dst_state = MockState {
            consensus_heights: vec![5],
            ..Default::default()
        };
        let (worker, _, _) = make_worker(src_state, dst_state);
        let mut last = None;
        assert!(!worker.scan(&mut last).await.unwrap());
        assert_eq!(last, Some(5));
    }

    #[tokio::test]
    async fn scan_updates_last_scanned_to_max_height() {
        let dst_state = MockState {
            consensus_heights: vec![20, 15, 10, 5],
            ..Default::default()
        };
        let (worker, _, _) = make_worker(MockState::default(), dst_state);
        let mut last = Some(8u64);
        assert!(!worker.scan(&mut last).await.unwrap());
        assert_eq!(last, Some(20));
    }

    #[tokio::test]
    async fn worker_cancels_token_on_misbehaviour() {
        let src_state = MockState {
            check_results: HashMap::from([(5, CheckResult::Evidence)]),
            ..Default::default()
        };
        let dst_state = MockState {
            consensus_heights: vec![5],
            ..Default::default()
        };
        let (worker, _, _) = make_worker(src_state, dst_state);
        let token = worker.token.clone();
        assert!(!token.is_cancelled());
        worker.run().await.unwrap();
        assert!(token.is_cancelled());
    }
}
