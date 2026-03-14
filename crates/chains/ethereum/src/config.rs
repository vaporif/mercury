use std::path::PathBuf;
use std::time::Duration;

use alloy::primitives::Address;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct EthereumChainConfig {
    pub chain_id: u64,
    pub rpc_addr: String,
    pub ics26_router: String,
    pub key_file: PathBuf,
    #[serde(default = "default_block_time_secs")]
    pub block_time_secs: u64,
    /// Block number at which the `ICS26Router` contract was deployed.
    /// Used as a lower bound when scanning historical logs.
    // NOTE: an alternative state-based approach would read `prevSequenceSends`
    // storage slot via `eth_getStorageAt` to get the max sequence, then check
    // each via `getCommitment()`. That avoids config but costs N RPC calls.
    #[serde(default)]
    pub deployment_block: u64,
}

const fn default_block_time_secs() -> u64 {
    12
}

impl EthereumChainConfig {
    #[must_use]
    pub const fn block_time(&self) -> Duration {
        Duration::from_secs(self.block_time_secs)
    }

    pub fn validate(&self) -> eyre::Result<()> {
        if !self.rpc_addr.starts_with("http://") && !self.rpc_addr.starts_with("https://") {
            eyre::bail!(
                "ethereum chain '{}': rpc_addr must start with http:// or https://, got '{}'",
                self.chain_id,
                self.rpc_addr
            );
        }
        self.router_address()?;
        Ok(())
    }

    pub fn router_address(&self) -> eyre::Result<Address> {
        self.ics26_router
            .parse()
            .map_err(|e| eyre::eyre!("invalid ics26_router address: {e}"))
    }

    #[must_use]
    pub fn chain_id_str(&self) -> String {
        self.chain_id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let config: EthereumChainConfig = toml::from_str(
            r#"
            chain_id = 31337
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"
            "#,
        )
        .unwrap();

        assert_eq!(config.block_time(), Duration::from_secs(12));
    }

    #[test]
    fn validate_rejects_empty_rpc() {
        let config = EthereumChainConfig {
            chain_id: 1,
            rpc_addr: String::new(),
            ics26_router: "0x0000000000000000000000000000000000000001".to_string(),
            key_file: "key.hex".into(),
            block_time_secs: default_block_time_secs(),
            deployment_block: 0,
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_ws_rpc() {
        let config = EthereumChainConfig {
            chain_id: 1,
            rpc_addr: "ws://localhost:8545".to_string(),
            ics26_router: "0x0000000000000000000000000000000000000001".to_string(),
            key_file: "key.hex".into(),
            block_time_secs: default_block_time_secs(),
            deployment_block: 0,
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_bad_address() {
        let config = EthereumChainConfig {
            chain_id: 1,
            rpc_addr: "http://localhost:8545".to_string(),
            ics26_router: "not-an-address".to_string(),
            key_file: "key.hex".into(),
            block_time_secs: default_block_time_secs(),
            deployment_block: 0,
        };
        assert!(config.validate().is_err());
    }
}
