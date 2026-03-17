use std::any::Any;
use std::path::Path;
use std::sync::Arc;

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

#[derive(Debug, Clone)]
pub struct ChainStatusInfo {
    pub chain_id: String,
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
    pub clearing_interval_secs: Option<u64>,
    pub misbehaviour_scan_interval_secs: Option<u64>,
    pub packet_filter_config: Option<toml::Value>,
}

pub trait ChainPlugin: Send + Sync {
    fn chain_type(&self) -> &'static str;
    fn validate_config(&self, raw: &toml::Table) -> eyre::Result<()>;
    fn connect(
        &self,
        raw_config: &toml::Table,
        config_dir: &Path,
    ) -> BoxFuture<'_, eyre::Result<AnyChain>>;
    fn parse_client_id(&self, raw: &str) -> eyre::Result<AnyClientId>;
    fn query_status(&self, chain: &AnyChain) -> BoxFuture<'_, eyre::Result<ChainStatusInfo>>;
    fn chain_id_from_config(&self, raw: &toml::Table) -> eyre::Result<String>;
    fn rpc_addr_from_config(&self, raw: &toml::Table) -> eyre::Result<String>;
}

pub trait RelayPairPlugin: Send + Sync {
    fn src_type(&self) -> &'static str;
    fn dst_type(&self) -> &'static str;

    fn build_relay(
        &self,
        src: &AnyChain,
        dst: &AnyChain,
        src_client_id: &str,
        dst_client_id: &str,
    ) -> eyre::Result<(Arc<dyn DynRelay>, Arc<dyn DynRelay>)>;
}
