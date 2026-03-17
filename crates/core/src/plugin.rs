use std::any::Any;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;

#[cfg(unix)]
pub fn warn_key_file_permissions(key_path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(key_path) {
        let mode = meta.permissions().mode();
        if mode & 0o044 != 0 {
            tracing::warn!(
                path = %key_path.display(),
                "key file is readable by group/others — consider chmod 600"
            );
        }
    }
}

pub type AnyChain = Arc<dyn Any + Send + Sync>;

pub type AnyClientId = Box<dyn Any + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChainId(pub String);

impl std::fmt::Display for ChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ChainId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for ChainId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<String> for ChainId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ChainId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct ChainStatusInfo {
    pub chain_id: ChainId,
    pub height: u64,
    pub timestamp: String,
}

pub trait DynRelay: Send + Sync {
    fn run(
        self: Arc<Self>,
        token: tokio_util::sync::CancellationToken,
        config: DynRelayConfig,
    ) -> BoxFuture<'static, crate::error::Result<()>>;
}

#[derive(Clone, Default)]
pub struct DynRelayConfig {
    pub lookback_secs: Option<u64>,
    pub sweep_interval_secs: Option<u64>,
    pub misbehaviour_scan_interval_secs: Option<u64>,
    pub packet_filter_config: Option<toml::Value>,
}

#[async_trait]
pub trait ChainPlugin: Send + Sync {
    fn chain_type(&self) -> &'static str;
    fn validate_config(&self, raw: &toml::Table) -> eyre::Result<()>;
    async fn connect(&self, raw_config: &toml::Table, config_dir: &Path) -> eyre::Result<AnyChain>;
    fn parse_client_id(&self, raw: &str) -> eyre::Result<AnyClientId>;
    async fn query_status(&self, chain: &AnyChain) -> eyre::Result<ChainStatusInfo>;
    fn chain_id_from_config(&self, raw: &toml::Table) -> eyre::Result<ChainId>;
    fn rpc_addr_from_config(&self, raw: &toml::Table) -> eyre::Result<String>;
}

pub trait RelayPairPlugin: Send + Sync {
    fn src_type(&self) -> &'static str;
    fn dst_type(&self) -> &'static str;

    fn build_relay(
        &self,
        src: &AnyChain,
        dst: &AnyChain,
        src_client_id: &AnyClientId,
        dst_client_id: &AnyClientId,
    ) -> eyre::Result<(Arc<dyn DynRelay>, Arc<dyn DynRelay>)>;
}
