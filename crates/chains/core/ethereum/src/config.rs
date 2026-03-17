use std::path::PathBuf;
use std::time::Duration;

use alloy::primitives::Address;
use serde::Deserialize;

const fn default_quorum() -> usize {
    1
}

const fn default_proof_timeout_secs() -> u64 {
    120
}

const fn default_max_concurrent_proofs() -> usize {
    4
}

#[derive(Clone, Debug, Deserialize)]
pub struct Sp1ProverConfig {
    pub elf_dir: PathBuf,
    pub zk_algorithm: ZkAlgorithm,
    pub prover_mode: ProverMode,
    #[serde(default = "default_proof_timeout_secs")]
    pub proof_timeout_secs: u64,
    #[serde(default = "default_max_concurrent_proofs")]
    pub max_concurrent_proofs: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZkAlgorithm {
    Groth16,
    Plonk,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProverMode {
    Mock,
    Cpu,
    Network,
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
    Mock,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EthereumChainConfig {
    #[serde(default)]
    pub chain_name: Option<String>,
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
    #[serde(default)]
    pub sp1_prover: Option<Sp1ProverConfig>,
    #[serde(default = "mercury_core::rpc_guard::default_timeout_secs")]
    pub rpc_timeout_secs: u64,
    #[serde(default = "mercury_core::rpc_guard::default_rate_limit")]
    pub rpc_rate_limit: u64,
}

const fn default_block_time_secs() -> u64 {
    12
}

impl EthereumChainConfig {
    #[must_use]
    pub const fn rpc_config(&self) -> mercury_core::rpc_guard::RpcConfig {
        mercury_core::rpc_guard::RpcConfig {
            rpc_timeout: Duration::from_secs(self.rpc_timeout_secs),
            rate_limit: self.rpc_rate_limit,
        }
    }

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
            ClientPayloadMode::Mock => {}
        }
        if let Some(ref sp1) = self.sp1_prover {
            #[cfg(not(feature = "sp1"))]
            {
                let _ = sp1;
                eyre::bail!(
                    "ethereum chain '{}': sp1_prover is configured but the binary was built \
                     without the `sp1` feature — rebuild with `--features sp1`",
                    self.chain_id
                );
            }

            #[cfg(feature = "sp1")]
            {
                if self.light_client_address.is_none() {
                    eyre::bail!(
                        "ethereum chain '{}': light_client_address is required when sp1_prover is configured",
                        self.chain_id
                    );
                }
                if sp1.proof_timeout_secs == 0 {
                    eyre::bail!(
                        "ethereum chain '{}': proof_timeout_secs must be > 0",
                        self.chain_id
                    );
                }
                if sp1.max_concurrent_proofs == 0 {
                    eyre::bail!(
                        "ethereum chain '{}': max_concurrent_proofs must be > 0",
                        self.chain_id
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
            chain_name: None,
            chain_id: 1,
            rpc_addr: String::new(),
            ics26_router: "0x0000000000000000000000000000000000000001".to_string(),
            key_file: "key.hex".into(),
            block_time_secs: default_block_time_secs(),
            deployment_block: 0,
            light_client_address: None,
            client_payload_mode: test_payload_mode(),
            sp1_prover: None,
            rpc_timeout_secs: mercury_core::rpc_guard::default_timeout_secs(),
            rpc_rate_limit: mercury_core::rpc_guard::default_rate_limit(),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_ws_rpc() {
        let config = EthereumChainConfig {
            chain_name: None,
            chain_id: 1,
            rpc_addr: "ws://localhost:8545".to_string(),
            ics26_router: "0x0000000000000000000000000000000000000001".to_string(),
            key_file: "key.hex".into(),
            block_time_secs: default_block_time_secs(),
            deployment_block: 0,
            light_client_address: None,
            client_payload_mode: test_payload_mode(),
            sp1_prover: None,
            rpc_timeout_secs: mercury_core::rpc_guard::default_timeout_secs(),
            rpc_rate_limit: mercury_core::rpc_guard::default_rate_limit(),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_bad_address() {
        let config = EthereumChainConfig {
            chain_name: None,
            chain_id: 1,
            rpc_addr: "http://localhost:8545".to_string(),
            ics26_router: "not-an-address".to_string(),
            key_file: "key.hex".into(),
            block_time_secs: default_block_time_secs(),
            deployment_block: 0,
            light_client_address: None,
            client_payload_mode: test_payload_mode(),
            sp1_prover: None,
            rpc_timeout_secs: mercury_core::rpc_guard::default_timeout_secs(),
            rpc_rate_limit: mercury_core::rpc_guard::default_rate_limit(),
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
    fn deserialize_mock_payload_mode() {
        let config: EthereumChainConfig = toml::from_str(
            r#"
        chain_id = 31337
        rpc_addr = "http://localhost:8545"
        ics26_router = "0x0000000000000000000000000000000000000001"
        key_file = "key.hex"

        [client_payload_mode]
        type = "mock"
        "#,
        )
        .unwrap();

        assert!(matches!(
            config.client_payload_mode,
            ClientPayloadMode::Mock
        ));
        assert!(config.validate().is_ok());
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

    #[test]
    fn deserialize_sp1_prover_config() {
        let config: EthereumChainConfig = toml::from_str(
            r#"
            chain_id = 1
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"
            light_client_address = "0x0000000000000000000000000000000000000002"

            [client_payload_mode]
            type = "beacon"
            beacon_api_url = "http://localhost:5052"

            [sp1_prover]
            elf_dir = "/opt/mercury/elf"
            zk_algorithm = "groth16"
            prover_mode = "network"
            "#,
        )
        .unwrap();

        let sp1 = config.sp1_prover.expect("sp1_prover should be Some");
        assert!(matches!(sp1.zk_algorithm, ZkAlgorithm::Groth16));
        assert!(matches!(sp1.prover_mode, ProverMode::Network));
        assert_eq!(sp1.proof_timeout_secs, 120);
        assert_eq!(sp1.max_concurrent_proofs, 4);
    }

    #[test]
    fn rpc_config_defaults_from_toml() {
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
        let rpc_config = config.rpc_config();
        assert_eq!(
            rpc_config.rpc_timeout,
            Duration::from_secs(mercury_core::rpc_guard::RpcConfig::DEFAULT_TIMEOUT_SECS)
        );
        assert_eq!(
            rpc_config.rate_limit,
            mercury_core::rpc_guard::RpcConfig::DEFAULT_RATE_LIMIT
        );
    }

    #[test]
    fn rpc_config_custom_from_toml() {
        let toml_str = format!(
            r#"
            chain_id = 31337
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"
            rpc_timeout_secs = 60
            rpc_rate_limit = 50
            {BEACON_TOML_SECTION}
            "#
        );
        let config: EthereumChainConfig = toml::from_str(&toml_str).unwrap();
        let rpc_config = config.rpc_config();
        assert_eq!(rpc_config.rpc_timeout, Duration::from_secs(60));
        assert_eq!(rpc_config.rate_limit, 50);
    }

    #[test]
    fn rpc_config_zero_rate_limit_rejected() {
        let toml_str = format!(
            r#"
            chain_id = 31337
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"
            rpc_rate_limit = 0
            {BEACON_TOML_SECTION}
            "#
        );
        let config: EthereumChainConfig = toml::from_str(&toml_str).unwrap();
        assert!(config.rpc_config().validate().is_err());
    }

    #[test]
    fn sp1_prover_config_defaults_to_none() {
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

        assert!(config.sp1_prover.is_none());
    }

    #[test]
    fn sp1_prover_custom_values() {
        let config: EthereumChainConfig = toml::from_str(
            r#"
            chain_id = 1
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"

            [client_payload_mode]
            type = "beacon"
            beacon_api_url = "http://localhost:5052"

            [sp1_prover]
            elf_dir = "/tmp/elf"
            zk_algorithm = "plonk"
            prover_mode = "cpu"
            proof_timeout_secs = 300
            max_concurrent_proofs = 8
            "#,
        )
        .unwrap();

        let sp1 = config.sp1_prover.unwrap();
        assert!(matches!(sp1.zk_algorithm, ZkAlgorithm::Plonk));
        assert!(matches!(sp1.prover_mode, ProverMode::Cpu));
        assert_eq!(sp1.proof_timeout_secs, 300);
        assert_eq!(sp1.max_concurrent_proofs, 8);
    }

    #[cfg(feature = "sp1")]
    #[test]
    fn validate_rejects_sp1_without_light_client_address() {
        let config: EthereumChainConfig = toml::from_str(
            r#"
            chain_id = 1
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"

            [client_payload_mode]
            type = "beacon"
            beacon_api_url = "http://localhost:5052"

            [sp1_prover]
            elf_dir = "/tmp/elf"
            zk_algorithm = "groth16"
            prover_mode = "mock"
            "#,
        )
        .unwrap();

        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("light_client_address"),
            "expected light_client_address error, got: {err}"
        );
    }

    #[cfg(feature = "sp1")]
    #[test]
    fn validate_rejects_zero_proof_timeout() {
        let config: EthereumChainConfig = toml::from_str(
            r#"
            chain_id = 1
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"
            light_client_address = "0x0000000000000000000000000000000000000002"

            [client_payload_mode]
            type = "beacon"
            beacon_api_url = "http://localhost:5052"

            [sp1_prover]
            elf_dir = "/tmp/elf"
            zk_algorithm = "groth16"
            prover_mode = "mock"
            proof_timeout_secs = 0
            "#,
        )
        .unwrap();

        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("proof_timeout_secs"),
            "expected proof_timeout_secs error, got: {err}"
        );
    }

    #[cfg(feature = "sp1")]
    #[test]
    fn validate_rejects_zero_max_concurrent_proofs() {
        let config: EthereumChainConfig = toml::from_str(
            r#"
            chain_id = 1
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"
            light_client_address = "0x0000000000000000000000000000000000000002"

            [client_payload_mode]
            type = "beacon"
            beacon_api_url = "http://localhost:5052"

            [sp1_prover]
            elf_dir = "/tmp/elf"
            zk_algorithm = "groth16"
            prover_mode = "mock"
            max_concurrent_proofs = 0
            "#,
        )
        .unwrap();

        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("max_concurrent_proofs"),
            "expected max_concurrent_proofs error, got: {err}"
        );
    }

    #[test]
    fn validate_sp1_config_accepted_when_valid() {
        let config: EthereumChainConfig = toml::from_str(
            r#"
            chain_id = 1
            rpc_addr = "http://localhost:8545"
            ics26_router = "0x0000000000000000000000000000000000000001"
            key_file = "key.hex"
            light_client_address = "0x0000000000000000000000000000000000000002"

            [client_payload_mode]
            type = "beacon"
            beacon_api_url = "http://localhost:5052"

            [sp1_prover]
            elf_dir = "/tmp/elf"
            zk_algorithm = "groth16"
            prover_mode = "mock"
            "#,
        )
        .unwrap();

        #[cfg(not(feature = "sp1"))]
        {
            let err = config.validate().unwrap_err();
            assert!(
                err.to_string().contains("sp1"),
                "expected sp1 feature error, got: {err}"
            );
        }

        #[cfg(feature = "sp1")]
        assert!(config.validate().is_ok());
    }
}
