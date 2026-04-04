use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct SolanaChainConfig {
    pub rpc_addr: String,
    #[serde(default)]
    pub ws_addr: Option<String>,
    pub program_id: String,
    #[serde(default)]
    pub ics07_program_id: Option<String>,
    pub keypair_path: PathBuf,
    #[serde(default = "default_block_time")]
    pub block_time: Duration,
    #[serde(default = "mercury_core::rpc_guard::default_timeout_secs")]
    pub rpc_timeout_secs: u64,
    #[serde(default = "mercury_core::rpc_guard::default_rate_limit")]
    pub rpc_rate_limit: u64,
    #[serde(default)]
    pub alt_address: Option<String>,
    #[serde(default = "default_skip_pre_verify_threshold")]
    pub skip_pre_verify_threshold: Option<usize>,
}

const fn default_skip_pre_verify_threshold() -> Option<usize> {
    Some(50)
}

const fn default_block_time() -> Duration {
    Duration::from_millis(400)
}

impl SolanaChainConfig {
    #[must_use]
    pub const fn rpc_config(&self) -> mercury_core::rpc_guard::RpcConfig {
        mercury_core::rpc_guard::RpcConfig {
            rpc_timeout: Duration::from_secs(self.rpc_timeout_secs),
            rate_limit: self.rpc_rate_limit,
        }
    }

    pub fn validate(&self) -> eyre::Result<()> {
        use mercury_core::validate::require_http_url;
        require_http_url("rpc_addr", &self.rpc_addr)?;
        if let Some(ref ws) = self.ws_addr {
            mercury_core::validate::require_ws_url("ws_addr", ws)?;
        }
        eyre::ensure!(!self.program_id.is_empty(), "program_id must not be empty");
        if let Some(ref alt) = self.alt_address {
            alt.parse::<solana_sdk::pubkey::Pubkey>()
                .map_err(|e| eyre::eyre!("invalid alt_address: {e}"))?;
        }
        eyre::ensure!(
            self.keypair_path.exists(),
            "keypair_path does not exist: {}",
            self.keypair_path.display()
        );
        Ok(())
    }
}
