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

    async fn build_create_client_payload(
        &self,
        chain: &AnyChain,
    ) -> eyre::Result<Box<dyn Any + Send + Sync>>;

    async fn create_client(
        &self,
        chain: &AnyChain,
        payload: Box<dyn Any + Send + Sync>,
    ) -> eyre::Result<String>;

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

    /// Builds headers/proofs from the reference chain for a client update.
    /// Ethereum beacon mode needs `counterparty_client_state` to read the trusted slot.
    async fn build_update_client_payload(
        &self,
        chain: &AnyChain,
        trusted_height: u64,
        target_height: u64,
        counterparty_client_state: Option<&(dyn Any + Send + Sync)>,
    ) -> eyre::Result<Box<dyn Any + Send + Sync>>;

    async fn update_client(
        &self,
        chain: &AnyChain,
        client_id: &str,
        payload: Box<dyn Any + Send + Sync>,
    ) -> eyre::Result<()>;
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn chain_id_conversions_and_display_agree() {
        let from_str = ChainId::from("cosmoshub-4");
        let from_string = ChainId::from("cosmoshub-4".to_string());
        assert_eq!(from_str, from_string);
        assert_eq!(from_str.to_string(), "cosmoshub-4");
        assert_eq!(from_str.as_ref(), "cosmoshub-4");
    }

    #[test]
    fn chain_id_keyed_map_is_queryable_by_str() {
        // Borrow<str> must hash/compare like the String inside, or lookups by
        // relay config names silently miss.
        let mut map = HashMap::new();
        map.insert(ChainId::from("osmosis-1"), 1);
        assert_eq!(map.get("osmosis-1"), Some(&1));
        assert_eq!(map.get("osmosis-2"), None);
    }

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Write for Buf {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[cfg(unix)]
    fn warnings_for(path: &Path) -> String {
        let buf = Buf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || warn_key_file_permissions(path));
        String::from_utf8(buf.0.lock().unwrap().clone()).unwrap()
    }

    #[cfg(unix)]
    fn key_file_with_mode(mode: u32) -> tempfile::NamedTempFile {
        use std::os::unix::fs::PermissionsExt;
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(mode)).unwrap();
        file
    }

    #[cfg(unix)]
    #[test]
    fn key_file_readable_by_group_or_others_warns() {
        for mode in [0o640, 0o604, 0o644] {
            let file = key_file_with_mode(mode);
            let logs = warnings_for(file.path());
            assert!(logs.contains("WARN"), "mode {mode:o}: {logs}");
            assert!(logs.contains("chmod 600"), "mode {mode:o}: {logs}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn owner_only_key_file_does_not_warn() {
        for mode in [0o600, 0o400, 0o700] {
            let file = key_file_with_mode(mode);
            assert_eq!(warnings_for(file.path()), "", "mode {mode:o}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn missing_key_file_is_silently_ignored() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(warnings_for(&dir.path().join("absent.key")), "");
    }
}
