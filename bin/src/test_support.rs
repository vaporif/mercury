use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::FutureExt;
use futures::future::BoxFuture;
use mercury_core::plugin::{
    AnyChain, AnyClientId, ChainId, ChainPlugin, ChainStatusInfo, ClearResult, ClientStateInfo,
    DynRelay, DynRelayConfig, SweepScope,
};

pub const EXAMPLE_CONFIG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/relayer.toml");

// Example config chain ids; their key files never exist in a temp dir, so
// `connect` fails fast while loading the signer instead of touching the network.
pub const HUB: &str = "cosmoshub-4";
pub const OSMO: &str = "osmosis-1";

pub struct TempConfig {
    _dir: tempfile::TempDir,
    pub path: PathBuf,
}

pub fn write_config(contents: &str) -> TempConfig {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relayer.toml");
    std::fs::write(&path, contents).unwrap();
    TempConfig { _dir: dir, path }
}

pub fn example_config() -> TempConfig {
    write_config(&std::fs::read_to_string(EXAMPLE_CONFIG).unwrap())
}

pub fn cosmos_chain(chain_id: &str) -> String {
    format!(
        r#"
[[chains]]
type = "cosmos"
chain_id = "{chain_id}"
rpc_addr = "http://127.0.0.1:1"
grpc_addr = "http://127.0.0.1:1"
account_prefix = "cosmos"
key_name = "relayer"
key_file = "keys/{chain_id}.toml"
gas_price = {{ amount = 0.025, denom = "uatom" }}
"#
    )
}

pub fn missing_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("does-not-exist/relayer.toml")
}

pub fn err_string<T>(result: eyre::Result<T>) -> String {
    match result {
        Ok(_) => panic!("expected an error"),
        Err(e) => format!("{e:#}"),
    }
}

// Chain plugin whose chain answers `query_status` with `height`, or fails when
// `height` is None. Chain handles are the plugin's `Option<u64>` boxed as `AnyChain`.
pub struct StatusPlugin(pub &'static str);

pub fn status_chain(height: Option<u64>) -> AnyChain {
    Arc::new(height)
}

#[async_trait]
impl ChainPlugin for StatusPlugin {
    fn chain_type(&self) -> &'static str {
        self.0
    }
    fn validate_config(&self, _raw: &toml::Table) -> eyre::Result<()> {
        Ok(())
    }
    async fn connect(&self, _raw: &toml::Table, _dir: &Path) -> eyre::Result<AnyChain> {
        Ok(status_chain(Some(1)))
    }
    fn parse_client_id(&self, raw: &str) -> eyre::Result<AnyClientId> {
        Ok(Box::new(raw.to_string()))
    }
    async fn query_status(&self, chain: &AnyChain) -> eyre::Result<ChainStatusInfo> {
        let height = chain.downcast_ref::<Option<u64>>().unwrap();
        match height {
            Some(h) => Ok(ChainStatusInfo {
                chain_id: ChainId::from("unused"),
                height: *h,
                timestamp: format!("ts-{h}"),
            }),
            None => eyre::bail!("rpc down"),
        }
    }
    fn chain_id_from_config(&self, _raw: &toml::Table) -> eyre::Result<ChainId> {
        Ok(ChainId::from(self.0))
    }
    fn rpc_addr_from_config(&self, _raw: &toml::Table) -> eyre::Result<String> {
        Ok(String::new())
    }
    async fn build_create_client_payload(
        &self,
        _chain: &AnyChain,
    ) -> eyre::Result<Box<dyn Any + Send + Sync>> {
        eyre::bail!("unused")
    }
    async fn create_client(
        &self,
        _chain: &AnyChain,
        _payload: Box<dyn Any + Send + Sync>,
    ) -> eyre::Result<String> {
        eyre::bail!("unused")
    }
    async fn query_client_state_info(
        &self,
        _chain: &AnyChain,
        _client_id: &str,
        _height: Option<u64>,
    ) -> eyre::Result<ClientStateInfo> {
        eyre::bail!("unused")
    }
    async fn query_commitment_sequences(
        &self,
        _chain: &AnyChain,
        _client_id: &str,
        _height: Option<u64>,
    ) -> eyre::Result<Vec<u64>> {
        eyre::bail!("unused")
    }
    async fn build_update_client_payload(
        &self,
        _chain: &AnyChain,
        _trusted_height: u64,
        _target_height: u64,
        _counterparty_client_state: Option<&(dyn Any + Send + Sync)>,
    ) -> eyre::Result<Box<dyn Any + Send + Sync>> {
        eyre::bail!("unused")
    }
    async fn update_client(
        &self,
        _chain: &AnyChain,
        _client_id: &str,
        _payload: Box<dyn Any + Send + Sync>,
    ) -> eyre::Result<()> {
        eyre::bail!("unused")
    }
}

// Relay direction that records the config it ran with and returns `outcome`.
pub struct ScriptedRelay {
    outcome: Option<&'static str>,
    pub seen: Mutex<Option<DynRelayConfig>>,
}

impl ScriptedRelay {
    pub fn ok() -> Arc<Self> {
        Arc::new(Self {
            outcome: None,
            seen: Mutex::new(None),
        })
    }

    pub fn failing(msg: &'static str) -> Arc<Self> {
        Arc::new(Self {
            outcome: Some(msg),
            seen: Mutex::new(None),
        })
    }
}

impl DynRelay for ScriptedRelay {
    fn clear_packets(
        self: Arc<Self>,
        _scope: SweepScope,
    ) -> BoxFuture<'static, mercury_core::error::Result<ClearResult>> {
        async { Ok(ClearResult::default()) }.boxed()
    }

    fn run(
        self: Arc<Self>,
        _token: tokio_util::sync::CancellationToken,
        config: DynRelayConfig,
    ) -> BoxFuture<'static, mercury_core::error::Result<()>> {
        async move {
            *self.seen.lock().unwrap() = Some(config);
            self.outcome.map_or(Ok(()), |msg| Err(eyre::eyre!(msg)))
        }
        .boxed()
    }
}
