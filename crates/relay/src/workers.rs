pub mod client_refresh;
pub mod event_watcher;
pub mod misbehaviour_worker;
pub mod packet_sweeper;
pub mod packet_worker;
pub mod tx_worker;

use std::time::{Duration, Instant};

use tracing::debug;

use mercury_chain_traits::queries::ChainStatusQuery;
use mercury_chain_traits::relay::Relay;
use mercury_chain_traits::types::ChainTypes;

const DST_CATCHUP_POLL_INTERVAL: Duration = Duration::from_secs(5);
const DST_CATCHUP_TIMEOUT: Duration = Duration::from_mins(15);

/// Polls dst chain until its timestamp catches up to `required_ts`.
pub(crate) async fn wait_for_dst_timestamp<R: Relay>(
    dst_chain: &R::DstChain,
    required_ts: u64,
) -> eyre::Result<()> {
    let start = tokio::time::Instant::now();
    loop {
        let dst_status = dst_chain.query_chain_status().await?;
        let dst_ts = <R::DstChain as ChainTypes>::chain_status_timestamp_secs(&dst_status);
        if dst_ts >= required_ts {
            return Ok(());
        }

        if start.elapsed() > DST_CATCHUP_TIMEOUT {
            eyre::bail!(
                "destination chain timestamp {dst_ts} did not reach \
                 required {required_ts} within {}s",
                DST_CATCHUP_TIMEOUT.as_secs()
            );
        }

        debug!(
            dst_ts,
            required_ts, "waiting for destination chain to catch up"
        );
        tokio::time::sleep(DST_CATCHUP_POLL_INTERVAL).await;
    }
}

pub struct TimestampedMessages<M> {
    pub messages: Vec<M>,
    pub created_at: Instant,
}

pub struct DstTxRequest<R: Relay> {
    pub messages: Vec<<R::DstChain as ChainTypes>::Message>,
    pub created_at: Instant,
}

impl<R: Relay> From<DstTxRequest<R>> for TimestampedMessages<<R::DstChain as ChainTypes>::Message> {
    fn from(req: DstTxRequest<R>) -> Self {
        Self {
            messages: req.messages,
            created_at: req.created_at,
        }
    }
}

pub struct SrcTxRequest<R: Relay> {
    pub messages: Vec<<R::SrcChain as ChainTypes>::Message>,
    pub created_at: Instant,
}

impl<R: Relay> From<SrcTxRequest<R>> for TimestampedMessages<<R::SrcChain as ChainTypes>::Message> {
    fn from(req: SrcTxRequest<R>) -> Self {
        Self {
            messages: req.messages,
            created_at: req.created_at,
        }
    }
}
