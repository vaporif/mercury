use std::path::PathBuf;

use eyre::Result;
use ibc::core::host::types::identifiers::ClientId;
use mercury_cosmos_counterparties::CosmosAdapter;
use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
use mercury_ethereum::config::{ClientPayloadMode, EthereumChainConfig};
use mercury_ethereum::types::EvmClientId;
use mercury_ethereum_counterparties::EthereumAdapter;
use tokio::sync::OnceCell;
use tracing::info;

use crate::bootstrap::anvil::DeployedContracts;
use crate::bootstrap::cosmos_docker::CosmosDockerHandle;

pub struct BeaconLcTestContext {
    pub cosmos_handle: CosmosDockerHandle,
    pub cosmos_chain: CosmosAdapter<Secp256k1KeyPair>,
    pub eth_chain: EthereumAdapter,
    pub client_id_on_cosmos: ClientId,
    pub client_id_on_eth: EvmClientId,
}

static SHARED_CONTRACTS: OnceCell<DeployedContracts> = OnceCell::const_new();

impl BeaconLcTestContext {
    #[allow(clippy::future_not_send, clippy::too_many_lines)]
    pub async fn setup() -> Result<Self> {
        let kurtosis = crate::bootstrap::kurtosis::get_or_init_kurtosis().await?;

        let eureka_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../external/solidity-ibc-eureka");
        let contracts = SHARED_CONTRACTS
            .get_or_try_init(|| async {
                crate::bootstrap::anvil::deploy_ibc_contracts(
                    &kurtosis.el_rpc_url,
                    &kurtosis.pre_funded_key.private_key,
                    kurtosis.chain_id,
                    &eureka_dir,
                    &kurtosis.pre_funded_key.address,
                )
            })
            .await?;

        let real_wasm_path = crate::bootstrap::wasm_lc::build_real_wasm_lc();

        let (cosmos_handle, _wasm_checksum, cosmos_chain) =
            crate::cosmos_eth_context::bootstrap_cosmos(real_wasm_path, false).await?;

        info!("building SP1 programs and deriving vkeys");
        let elf_dir = crate::bootstrap::anvil::build_sp1_programs()?;
        let vkeys = crate::bootstrap::anvil::derive_sp1_vkeys(&elf_dir)?;

        let (client_state_abi, consensus_state_hash) =
            crate::cosmos_eth_context::build_sp1_client_state(&cosmos_handle).await?;

        let sp1_light_client = crate::bootstrap::anvil::deploy_sp1_light_client(
            &kurtosis.el_rpc_url,
            &kurtosis.pre_funded_key.private_key,
            contracts.mock_verifier,
            &vkeys,
            &client_state_abi,
            consensus_state_hash,
        )?;

        let eth_signer: alloy::signers::local::PrivateKeySigner = kurtosis
            .pre_funded_key
            .private_key
            .parse()
            .map_err(|e| eyre::eyre!("parsing kurtosis key: {e}"))?;

        let eth_config = EthereumChainConfig {
            chain_name: Some("kurtosis-ethereum".into()),
            chain_id: kurtosis.chain_id,
            rpc_addr: kurtosis.el_rpc_url.clone(),
            ws_addr: None,
            ics26_router: format!("{:#x}", contracts.ics26_router),
            key_file: PathBuf::new(),
            block_time_secs: 2,
            deployment_block: 0,
            light_client_address: Some(format!("{sp1_light_client:#x}")),
            client_payload_mode: ClientPayloadMode::Beacon {
                beacon_api_url: kurtosis.beacon_api_url.clone(),
            },
            sp1_prover: Some(mercury_ethereum::config::Sp1ProverConfig {
                elf_dir,
                zk_algorithm: mercury_ethereum::config::ZkAlgorithm::Groth16,
                prover_mode: mercury_ethereum::config::ProverMode::Mock,
                proof_timeout_secs: 120,
                max_concurrent_proofs: 4,
            }),
            rpc_timeout_secs: mercury_core::rpc_guard::default_timeout_secs(),
            rpc_rate_limit: mercury_core::rpc_guard::default_rate_limit(),
            gas_multiplier: None,
            max_gas: None,
            max_priority_fee_multiplier: None,
        };

        let eth_chain = EthereumAdapter::new(eth_config, eth_signer)
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        let (client_id_on_cosmos, client_id_on_eth) =
            crate::cosmos_eth_context::create_ibc_clients(&cosmos_chain, &eth_chain).await?;

        info!("Beacon LC test setup complete");

        Ok(Self {
            cosmos_handle,
            cosmos_chain,
            eth_chain,
            client_id_on_cosmos,
            client_id_on_eth,
        })
    }
}
