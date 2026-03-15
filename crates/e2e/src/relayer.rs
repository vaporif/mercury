use std::process::{Child, Command};
use std::sync::Arc;
use std::time::Duration;

use eyre::{Context, Result, bail};
use mercury_relay::context::{RelayContext, RelayWorkerConfig};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::bootstrap::traits::ChainHandle;
use crate::context::TestContext;

pub struct RelayHandle {
    pub cancel: CancellationToken,
    pub join_ab: JoinHandle<mercury_core::error::Result<()>>,
    pub join_ba: JoinHandle<mercury_core::error::Result<()>>,
}

impl Drop for RelayHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.join_ab.abort();
        self.join_ba.abort();
    }
}

impl RelayHandle {
    pub fn stop(self) -> Result<()> {
        // cancel + abort happens in Drop
        Ok(())
    }
}

pub struct SubprocessHandle {
    child: Child,
    health_port: u16,
    _config_dir: tempfile::TempDir,
}

impl SubprocessHandle {
    /// Check if the subprocess is still running.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Poll the relayer's health endpoint until it responds HTTP 200.
    pub async fn wait_until_ready(&mut self, timeout: Duration) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        let start = std::time::Instant::now();
        let poll_interval = Duration::from_millis(250);
        let warning_threshold = Duration::from_secs(15);
        let mut warned = false;
        let addr = format!("127.0.0.1:{}", self.health_port);

        loop {
            let elapsed = start.elapsed();
            if elapsed > timeout {
                bail!("relayer health endpoint not ready after {elapsed:?}");
            }

            if !warned && elapsed > warning_threshold {
                warn!(
                    elapsed = ?elapsed,
                    "relayer taking longer than expected to become ready"
                );
                warned = true;
            }

            if !self.is_running() {
                bail!("relayer process exited unexpectedly during startup");
            }

            if let Ok(mut stream) = TcpStream::connect(&addr).await {
                let _ = stream
                    .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                    .await;
                let mut buf = [0u8; 32];
                if let Ok(n) = stream.read(&mut buf).await
                    && n > 0
                    && buf.starts_with(b"HTTP/1.1 200")
                {
                    info!(
                        elapsed = ?start.elapsed(),
                        "binary relayer ready (health check passed)"
                    );
                    return Ok(());
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    pub fn stop(mut self) -> Result<()> {
        self.child.kill().wrap_err("killing mercury-relayer")?;
        self.child.wait().wrap_err("waiting for mercury-relayer")?;
        Ok(())
    }
}

impl Drop for SubprocessHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl TestContext {
    /// Start mercury relay workers in-process (bidirectional).
    pub fn start_relay_library(&self) -> Result<RelayHandle> {
        let token = CancellationToken::new();

        let relay_ab = Arc::new(RelayContext {
            src_chain: self.cosmos_a.clone(),
            dst_chain: self.cosmos_b.clone(),
            src_client_id: self.client_id_a.clone(),
            dst_client_id: self.client_id_b.clone(),
        });

        let relay_ba = Arc::new(RelayContext {
            src_chain: self.cosmos_b.clone(),
            dst_chain: self.cosmos_a.clone(),
            src_client_id: self.client_id_b.clone(),
            dst_client_id: self.client_id_a.clone(),
        });

        let token_ab = token.clone();
        let token_ba = token.clone();

        let join_ab = tokio::spawn(async move {
            relay_ab
                .run_with_token(token_ab, RelayWorkerConfig::default())
                .await
        });
        let join_ba = tokio::spawn(async move {
            relay_ba
                .run_with_token(token_ba, RelayWorkerConfig::default())
                .await
        });

        Ok(RelayHandle {
            cancel: token,
            join_ab,
            join_ba,
        })
    }

    /// Start mercury-relayer as a subprocess.
    #[allow(clippy::missing_panics_doc)]
    pub fn start_relay_binary(&self) -> Result<SubprocessHandle> {
        let config_dir = tempfile::tempdir().wrap_err("creating temp dir")?;
        let config_path = config_dir.path().join("relayer.toml");

        self.write_relay_config(&config_dir, &config_path)?;

        let binary = find_or_build_binary();

        // Bind to port 0 to get a free port, then release it for the subprocess.
        let health_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0")
                .wrap_err("finding free port for health check")?;
            listener.local_addr().wrap_err("getting local addr")?.port()
        };

        let child = Command::new(&binary)
            .args([
                "start",
                "--config",
                &config_path.to_string_lossy(),
                "--health-port",
                &health_port.to_string(),
            ])
            .spawn()
            .wrap_err("spawning mercury-relayer")?;

        Ok(SubprocessHandle {
            child,
            health_port,
            _config_dir: config_dir,
        })
    }

    fn write_relay_config(
        &self,
        config_dir: &tempfile::TempDir,
        config_path: &std::path::Path,
    ) -> Result<()> {
        let key_path_a = config_dir.path().join("key_a.toml");
        let key_path_b = config_dir.path().join("key_b.toml");
        std::fs::write(
            &key_path_a,
            format!(
                "secret_key = \"{}\"",
                self.handle_a.relayer_wallet().secret_key_hex
            ),
        )?;
        std::fs::write(
            &key_path_b,
            format!(
                "secret_key = \"{}\"",
                self.handle_b.relayer_wallet().secret_key_hex
            ),
        )?;

        let config = format!(
            r#"
[[chains]]
type = "cosmos"
chain_id = "{chain_id_a}"
rpc_addr = "{rpc_a}"
grpc_addr = "{grpc_a}"
account_prefix = "cosmos"
key_name = "relayer"
key_file = "{key_a}"
[chains.gas_price]
amount = 0.0
denom = "stake"

[[chains]]
type = "cosmos"
chain_id = "{chain_id_b}"
rpc_addr = "{rpc_b}"
grpc_addr = "{grpc_b}"
account_prefix = "cosmos"
key_name = "relayer"
key_file = "{key_b}"
[chains.gas_price]
amount = 0.0
denom = "stake"

[[relays]]
src_chain = "{chain_id_a}"
dst_chain = "{chain_id_b}"
src_client_id = "{client_a}"
dst_client_id = "{client_b}"
"#,
            chain_id_a = self.handle_a.chain_id(),
            rpc_a = self.handle_a.rpc_endpoint(),
            grpc_a = self.handle_a.grpc_endpoint(),
            key_a = key_path_a.display(),
            chain_id_b = self.handle_b.chain_id(),
            rpc_b = self.handle_b.rpc_endpoint(),
            grpc_b = self.handle_b.grpc_endpoint(),
            key_b = key_path_b.display(),
            client_a = self.client_id_a,
            client_b = self.client_id_b,
        );
        std::fs::write(config_path, config)?;
        Ok(())
    }
}

fn find_or_build_binary() -> String {
    std::env::var("MERCURY_RELAYER_BIN").unwrap_or_else(|_| {
        let output = Command::new("cargo")
            .args(["build", "-p", "mercury-cli", "--message-format=json"])
            .output()
            .expect("failed to run cargo build");
        assert!(output.status.success(), "cargo build failed");
        String::from_utf8(output.stdout)
            .expect("invalid utf8")
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .rfind(|v| v.get("executable").and_then(|e| e.as_str()).is_some())
            .and_then(|v| {
                v.get("executable")
                    .and_then(|e| e.as_str())
                    .map(String::from)
            })
            .expect("no executable found in cargo build output")
    })
}
