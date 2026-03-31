use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;

use crate::ThreadSafeAny;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ClientMode {
    #[default]
    Default,
    Native,
    Zk,
    Attested,
    Optimistic,
    Mock,
    /// CLI: `--mode trusted-execution`, TOML: `mode = "trusted_execution"`
    #[value(name = "trusted-execution")]
    TrustedExecution,
    Multisig,
    Proxy,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChainPair {
    pub src_type: String,
    pub dst_type: String,
    pub mode: ClientMode,
}

impl ChainPair {
    pub fn new(src_type: impl Into<String>, dst_type: impl Into<String>, mode: ClientMode) -> Self {
        Self {
            src_type: src_type.into(),
            dst_type: dst_type.into(),
            mode,
        }
    }
}

// TODO: consider typed errors (e.g. ClientBuilderError with UnsupportedOperation variant)
//       once callers need to branch on error kind
#[async_trait]
pub trait ClientBuilder: Send + Sync {
    async fn build_create_payload(&self, src_chain: &AnyChain) -> eyre::Result<Box<ThreadSafeAny>>;

    async fn create_client(
        &self,
        host_chain: &AnyChain,
        payload: Box<ThreadSafeAny>,
    ) -> eyre::Result<String>;

    async fn build_update_payload(
        &self,
        src_chain: &AnyChain,
        trusted_height: u64,
        target_height: u64,
        counterparty_client_state: Option<&ThreadSafeAny>,
    ) -> eyre::Result<Box<ThreadSafeAny>>;

    async fn update_client(
        &self,
        host_chain: &AnyChain,
        client_id: &str,
        payload: Box<ThreadSafeAny>,
    ) -> eyre::Result<()>;
}

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

pub type AnyChain = Arc<ThreadSafeAny>;
pub type AnyClientId = Box<ThreadSafeAny>;

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

#[derive(Debug, Clone)]
pub struct ClientStateInfo {
    pub client_id: String,
    pub latest_height: u64,
    pub trusting_period: Option<std::time::Duration>,
    pub frozen: bool,
    pub client_type: String,
    pub chain_id: String,
}

#[derive(Clone, Debug)]
pub enum SweepScope {
    All,
    Sequences(Vec<u64>),
}

#[derive(Clone, Debug, Default)]
pub struct ClearResult {
    pub recv_cleared: usize,
    pub ack_cleared: usize,
}

pub trait DynRelay: Send + Sync {
    fn clear_packets(
        self: Arc<Self>,
        scope: SweepScope,
    ) -> BoxFuture<'static, crate::error::Result<ClearResult>>;

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
    pub clear_on_start: bool,
    pub clear_limit: usize,
    pub excluded_sequences: Vec<u64>,
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

    async fn query_client_state_info(
        &self,
        chain: &AnyChain,
        client_id: &str,
        height: Option<u64>,
    ) -> eyre::Result<ClientStateInfo>;

    async fn query_commitment_sequences(
        &self,
        chain: &AnyChain,
        client_id: &str,
        height: Option<u64>,
    ) -> eyre::Result<Vec<u64>>;
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
