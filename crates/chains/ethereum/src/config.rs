use std::path::PathBuf;
use std::time::Duration;

use alloy::primitives::Address;
use serde::Deserialize;

const fn default_quorum() -> usize {
    1
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientPayloadMode {
    Beacon {
        beacon_api_url: String,
    },
    Attested {
        attestor_endpoints: Vec<String>,
        #[serde(default = "default_quorum")]
        quorum_threshold: usize,
    },
}

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
    #[serde(default)]
    pub light_client_address: Option<String>,
    pub client_payload_mode: ClientPayloadMode,
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
        if let Some(ref addr) = self.light_client_address {
            addr.parse::<Address>()
                .map_err(|e| eyre::eyre!("invalid light_client_address: {e}"))?;
        }
        match &self.client_payload_mode {
            ClientPayloadMode::Beacon { beacon_api_url } => {
                if !beacon_api_url.starts_with("http://") && !beacon_api_url.starts_with("https://")
                {
                    eyre::bail!(
                        "ethereum chain '{}': beacon_api_url must start with http:// or https://, got '{}'",
                        self.chain_id,
                        beacon_api_url
                    );
                }
            }
            ClientPayloadMode::Attested {
                attestor_endpoints,
                quorum_threshold,
            } => {
                if attestor_endpoints.is_empty() {
                    eyre::bail!(
                        "ethereum chain '{}': attestor_endpoints must not be empty",
                        self.chain_id
                    );
                }
                if *quorum_threshold == 0 || *quorum_threshold > attestor_endpoints.len() {
                    eyre::bail!(
                        "ethereum chain '{}': quorum_threshold must be 1..={}",
                        self.chain_id,
                        attestor_endpoints.len()
                    );
                }
            }
        }
        Ok(())
    }

    pub fn router_address(&self) -> eyre::Result<Address> {
        self.ics26_router
            .parse()
            .map_err(|e| eyre::eyre!("invalid ics26_router address: {e}"))
    }

    pub fn light_client_address(&self) -> eyre::Result<Address> {
        self.light_client_address
            .as_ref()
            .ok_or_else(|| eyre::eyre!("light_client_address not configured"))?
            .parse()
            .map_err(|e| eyre::eyre!("invalid light_client_address: {e}"))
    }

    #[must_use]
    pub fn chain_id_str(&self) -> String {
        self.chain_id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_payload_mode() -> ClientPayloadMode {
        ClientPayloadMode::Beacon {
            beacon_api_url: "http://localhost:5052".to_string(),
        }
    }

    const BEACON_TOML_SECTION: &str = r#"
            [client_payload_mode]
            type = "beacon"
            beacon_api_url = "http://localhost:5052"
    "#;

    #[test]
    fn default_values() {
        let toml_str = format!(
            r#"
            chain_id = 31337
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"
            {BEACON_TOML_SECTION}
            "#
        );
        let config: EthereumChainConfig = toml::from_str(&toml_str).unwrap();
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
            light_client_address: None,
            client_payload_mode: test_payload_mode(),
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
            light_client_address: None,
            client_payload_mode: test_payload_mode(),
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
            light_client_address: None,
            client_payload_mode: test_payload_mode(),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_accepts_valid_light_client_address() {
        let toml_str = format!(
            r#"
            chain_id = 31337
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"
            light_client_address = "0x0000000000000000000000000000000000000002"
            {BEACON_TOML_SECTION}
            "#
        );
        let config: EthereumChainConfig = toml::from_str(&toml_str).unwrap();
        assert!(config.validate().is_ok());
        assert!(config.light_client_address().is_ok());
    }

    #[test]
    fn validate_rejects_bad_light_client_address() {
        let toml_str = format!(
            r#"
            chain_id = 31337
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"
            light_client_address = "not-an-address"
            {BEACON_TOML_SECTION}
            "#
        );
        let config: EthereumChainConfig = toml::from_str(&toml_str).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn light_client_address_errors_when_none() {
        let toml_str = format!(
            r#"
            chain_id = 31337
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"
            {BEACON_TOML_SECTION}
            "#
        );
        let config: EthereumChainConfig = toml::from_str(&toml_str).unwrap();
        assert!(config.light_client_address().is_err());
    }

    #[test]
    fn deserialize_beacon_payload_mode() {
        let config: EthereumChainConfig = toml::from_str(
            r#"
            chain_id = 1
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"

            [client_payload_mode]
            type = "beacon"
            beacon_api_url = "http://localhost:5052"
            "#,
        )
        .unwrap();

        assert!(matches!(
            config.client_payload_mode,
            ClientPayloadMode::Beacon { .. }
        ));
    }

    #[test]
    fn deserialize_attested_payload_mode() {
        let config: EthereumChainConfig = toml::from_str(
            r#"
            chain_id = 1
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"

            [client_payload_mode]
            type = "attested"
            attestor_endpoints = ["http://attestor1:8080", "http://attestor2:8080"]
            quorum_threshold = 2
            "#,
        )
        .unwrap();

        assert!(matches!(
            config.client_payload_mode,
            ClientPayloadMode::Attested {
                quorum_threshold: 2,
                ..
            }
        ));
    }

    #[test]
    fn validate_rejects_bad_beacon_url() {
        let config: EthereumChainConfig = toml::from_str(
            r#"
            chain_id = 1
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"

            [client_payload_mode]
            type = "beacon"
            beacon_api_url = "not-a-url"
            "#,
        )
        .unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_attestor_endpoints() {
        let config: EthereumChainConfig = toml::from_str(
            r#"
            chain_id = 1
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"

            [client_payload_mode]
            type = "attested"
            attestor_endpoints = []
            quorum_threshold = 0
            "#,
        )
        .unwrap();
        assert!(config.validate().is_err());
    }
}
