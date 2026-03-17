use std::future::Future;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use metrics::{Counter, Histogram, counter, histogram};

use crate::error::{RpcError, Result};

pub const RPC_REQUESTS_TOTAL: &str = "rpc_requests_total";
pub const RPC_REQUEST_DURATION_MS: &str = "rpc_request_duration_ms";
pub const RPC_ERRORS_TOTAL: &str = "rpc_errors_total";
pub const RPC_TIMEOUTS_TOTAL: &str = "rpc_timeouts_total";
pub const RPC_RATE_LIMIT_WAIT_MS: &str = "rpc_rate_limit_wait_ms";

#[derive(Clone, Debug)]
pub struct RpcConfig {
    pub rpc_timeout: Duration,
    pub rate_limit: u64,
}

impl RpcConfig {
    pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
    pub const DEFAULT_RATE_LIMIT: u64 = 100;

    pub fn validate(&self) -> Result<()> {
        eyre::ensure!(self.rate_limit > 0, "rpc_rate_limit must be > 0");
        Ok(())
    }
}

/// Serde default helper for `rpc_timeout_secs` fields.
#[must_use]
pub const fn default_timeout_secs() -> u64 {
    RpcConfig::DEFAULT_TIMEOUT_SECS
}

/// Serde default helper for `rpc_rate_limit` fields.
#[must_use]
pub const fn default_rate_limit() -> u64 {
    RpcConfig::DEFAULT_RATE_LIMIT
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            rpc_timeout: Duration::from_secs(Self::DEFAULT_TIMEOUT_SECS),
            rate_limit: Self::DEFAULT_RATE_LIMIT,
        }
    }
}

type GovernorLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

#[derive(Clone)]
pub struct RpcGuard {
    config: RpcConfig,
    rate_limiter: Arc<GovernorLimiter>,
    chain_id: Arc<str>,
    m_requests: Counter,
    m_errors: Counter,
    m_timeouts: Counter,
    m_duration: Histogram,
    m_rate_wait: Histogram,
}

impl RpcGuard {
    /// # Panics
    /// This function will not panic in practice; the fallback ensures a valid `NonZeroU32`.
    #[must_use]
    pub fn new(chain_id: &str, config: RpcConfig) -> Self {
        let quota = Quota::per_second(
            NonZeroU32::new(config.rate_limit.try_into().unwrap_or(u32::MAX))
                .unwrap_or(NonZeroU32::new(1).expect("1 is non-zero")),
        );
        let chain: Arc<str> = chain_id.into();
        Self {
            config,
            rate_limiter: Arc::new(RateLimiter::direct(quota)),
            m_requests: counter!(RPC_REQUESTS_TOTAL, "chain" => chain.clone()),
            m_errors: counter!(RPC_ERRORS_TOTAL, "chain" => chain.clone()),
            m_timeouts: counter!(RPC_TIMEOUTS_TOTAL, "chain" => chain.clone()),
            m_duration: histogram!(RPC_REQUEST_DURATION_MS, "chain" => chain.clone()),
            m_rate_wait: histogram!(RPC_RATE_LIMIT_WAIT_MS, "chain" => chain.clone()),
            chain_id: chain,
        }
    }

    #[must_use]
    pub fn noop(chain_id: &str) -> Self {
        Self::new(chain_id, RpcConfig {
            rpc_timeout: Duration::from_secs(3600),
            rate_limit: u64::from(u32::MAX),
        })
    }

    pub async fn guarded<F, Fut, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let wait_start = std::time::Instant::now();
        self.rate_limiter.until_ready().await;
        let wait_ms = wait_start.elapsed().as_secs_f64() * 1000.0;
        if wait_ms > 1.0 {
            self.m_rate_wait.record(wait_ms);
        }

        let start = std::time::Instant::now();
        let result = tokio::time::timeout(self.config.rpc_timeout, f()).await;
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

        match result {
            Ok(Ok(value)) => {
                self.m_requests.increment(1);
                self.m_duration.record(elapsed_ms);
                Ok(value)
            }
            Ok(Err(e)) => {
                self.m_requests.increment(1);
                self.m_errors.increment(1);
                self.m_duration.record(elapsed_ms);
                Err(e)
            }
            Err(_) => {
                self.m_requests.increment(1);
                self.m_timeouts.increment(1);
                Err(RpcError::Timeout(self.config.rpc_timeout).into())
            }
        }
    }

    /// Run two futures concurrently under a single timeout.
    /// Consumes one rate-limit token (the pair is one logical operation).
    pub async fn guarded_pair<F1, Fut1, T1, F2, Fut2, T2>(
        &self,
        f1: F1,
        f2: F2,
    ) -> Result<(T1, T2)>
    where
        F1: FnOnce() -> Fut1,
        Fut1: Future<Output = Result<T1>>,
        F2: FnOnce() -> Fut2,
        Fut2: Future<Output = Result<T2>>,
    {
        let wait_start = std::time::Instant::now();
        self.rate_limiter.until_ready().await;
        let wait_ms = wait_start.elapsed().as_secs_f64() * 1000.0;
        if wait_ms > 1.0 {
            self.m_rate_wait.record(wait_ms);
        }

        let start = std::time::Instant::now();
        let result = tokio::time::timeout(self.config.rpc_timeout, async {
            tokio::try_join!(f1(), f2())
        })
        .await;
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

        match result {
            Ok(Ok((v1, v2))) => {
                self.m_requests.increment(2);
                self.m_duration.record(elapsed_ms);
                Ok((v1, v2))
            }
            Ok(Err(e)) => {
                self.m_requests.increment(2);
                self.m_errors.increment(1);
                self.m_duration.record(elapsed_ms);
                Err(e)
            }
            Err(_) => {
                self.m_requests.increment(2);
                self.m_timeouts.increment(1);
                Err(RpcError::Timeout(self.config.rpc_timeout).into())
            }
        }
    }
}

impl std::fmt::Debug for RpcGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcGuard")
            .field("chain_id", &self.chain_id)
            .field("timeout", &self.config.rpc_timeout)
            .field("rate_limit", &self.config.rate_limit)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    #[test]
    fn rpc_config_defaults() {
        let config = RpcConfig::default();
        assert_eq!(
            config.rpc_timeout,
            Duration::from_secs(RpcConfig::DEFAULT_TIMEOUT_SECS)
        );
        assert_eq!(config.rate_limit, RpcConfig::DEFAULT_RATE_LIMIT);
    }

    #[test]
    fn rpc_config_zero_rate_limit_rejected() {
        let result = std::panic::catch_unwind(|| {
            RpcConfig {
                rpc_timeout: Duration::from_secs(30),
                rate_limit: 0,
            }
            .validate()
        });
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }

    #[tokio::test]
    async fn guarded_success() {
        let guard = RpcGuard::new("test-chain", RpcConfig::default());
        let result = guard.guarded(|| async { Ok::<_, eyre::Report>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn guarded_propagates_error() {
        let guard = RpcGuard::new("test-chain", RpcConfig::default());
        let result = guard
            .guarded(|| async { Err::<i32, _>(eyre::eyre!("rpc error")) })
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("rpc error"));
    }

    #[tokio::test]
    async fn guarded_timeout() {
        let config = RpcConfig {
            rpc_timeout: Duration::from_millis(200),
            rate_limit: 1000,
        };
        let guard = RpcGuard::new("test-chain", config);
        let start = std::time::Instant::now();
        let result = guard
            .guarded(|| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok::<_, eyre::Report>(())
            })
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .downcast_ref::<crate::error::RpcError>()
            .is_some());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn guarded_rate_limits() {
        let config = RpcConfig {
            rpc_timeout: Duration::from_secs(10),
            rate_limit: 10,
        };
        let guard = RpcGuard::new("test-chain", config);
        let counter = Arc::new(AtomicU32::new(0));
        let start = std::time::Instant::now();
        for _ in 0..15 {
            let c = counter.clone();
            guard
                .guarded(|| async move {
                    c.fetch_add(1, Ordering::Relaxed);
                    Ok::<_, eyre::Report>(())
                })
                .await
                .unwrap();
        }
        assert_eq!(counter.load(Ordering::Relaxed), 15);
        assert!(start.elapsed() >= Duration::from_millis(400));
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn guarded_pair_runs_concurrently() {
        let guard = RpcGuard::new("test-chain", RpcConfig::default());
        let (a, b) = guard
            .guarded_pair(
                || async { Ok::<_, eyre::Report>(1) },
                || async { Ok::<_, eyre::Report>(2) },
            )
            .await
            .unwrap();
        assert_eq!(a, 1);
        assert_eq!(b, 2);
    }

    #[tokio::test]
    async fn guarded_pair_timeout() {
        let config = RpcConfig {
            rpc_timeout: Duration::from_millis(200),
            rate_limit: 1000,
        };
        let guard = RpcGuard::new("test-chain", config);
        let result = guard
            .guarded_pair(
                || async { Ok::<_, eyre::Report>(1) },
                || async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok::<_, eyre::Report>(2)
                },
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cloned_guards_share_rate_limiter() {
        let config = RpcConfig {
            rpc_timeout: Duration::from_secs(10),
            rate_limit: 10,
        };
        let guard1 = RpcGuard::new("test", config);
        let guard2 = guard1.clone();
        let start = std::time::Instant::now();
        for _ in 0..10 {
            guard1
                .guarded(|| async { Ok::<_, eyre::Report>(()) })
                .await
                .unwrap();
        }
        for _ in 0..5 {
            guard2
                .guarded(|| async { Ok::<_, eyre::Report>(()) })
                .await
                .unwrap();
        }
        assert!(start.elapsed() >= Duration::from_millis(400));
    }

    #[tokio::test]
    async fn noop_guard_has_no_timeout_or_rate_limit() {
        let guard = RpcGuard::noop("test");
        let start = std::time::Instant::now();
        for _ in 0..100 {
            guard
                .guarded(|| async { Ok::<_, eyre::Report>(()) })
                .await
                .unwrap();
        }
        assert!(start.elapsed() < Duration::from_millis(100));
    }
}
