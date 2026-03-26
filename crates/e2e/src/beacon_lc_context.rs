use std::path::PathBuf;
use std::time::Duration;

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider as _, ProviderBuilder};
use eyre::Result;
use ibc::core::host::types::identifiers::ClientId;
use mercury_cosmos_counterparties::CosmosAdapter;
use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
use mercury_ethereum::config::{ClientPayloadMode, EthereumChainConfig};
use mercury_ethereum::types::EvmClientId;
use mercury_ethereum_counterparties::EthereumAdapter;
use tokio::sync::OnceCell;
use tracing::info;

use crate::bootstrap::anvil::{AnvilWallet, DeployedContracts};
use crate::bootstrap::cosmos_docker::CosmosDockerHandle;
use crate::bootstrap::kurtosis::KurtosisHandle;
use crate::bootstrap::traits::ChainHandle;
use crate::cosmos_eth_context::{
    poll_cosmos_balance, poll_eth_balance, query_cosmos_bank_balance, query_erc20_balance,
    start_cross_chain_relay, submit_cosmos_to_eth_transfer, submit_eth_to_cosmos_transfer,
};
use crate::relayer::RelayHandle;

pub struct BeaconLcTestContext {
    pub cosmos_handle: CosmosDockerHandle,
    pub cosmos_chain: CosmosAdapter<Secp256k1KeyPair>,
    pub eth_chain: EthereumAdapter,
    pub client_id_on_cosmos: ClientId,
    pub client_id_on_eth: EvmClientId,
    pub el_rpc_url: String,
    pub ics20_transfer: Address,
    pub eth_user_wallet: AnvilWallet,
}

static SHARED_CONTRACTS: OnceCell<DeployedContracts> = OnceCell::const_new();

impl BeaconLcTestContext {
    #[allow(clippy::future_not_send, clippy::too_many_lines)]
    pub async fn setup() -> Result<Self> {
        let kurtosis = crate::bootstrap::kurtosis::get_or_init_kurtosis().await?;

        let eureka_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../external/solidity-ibc-eureka");
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

        let eth_user_wallet = fund_eth_user_wallet(kurtosis).await?;

        let (client_id_on_cosmos, client_id_on_eth) =
            crate::cosmos_eth_context::create_ibc_clients(&cosmos_chain, &eth_chain).await?;

        info!("Beacon LC test setup complete");

        Ok(Self {
            cosmos_handle,
            cosmos_chain,
            eth_chain,
            client_id_on_cosmos,
            client_id_on_eth,
            el_rpc_url: kurtosis.el_rpc_url.clone(),
            ics20_transfer: contracts.ics20_transfer,
            eth_user_wallet,
        })
    }

    pub fn start_relay_library(&self) -> Result<RelayHandle> {
        start_cross_chain_relay(
            &self.cosmos_chain,
            &self.eth_chain,
            &self.client_id_on_cosmos,
            &self.client_id_on_eth,
        )
    }

    #[allow(clippy::future_not_send)]
    pub async fn send_cosmos_to_eth_transfer(&self, amount: u64, denom: &str) -> Result<()> {
        submit_cosmos_to_eth_transfer(
            &self.cosmos_handle,
            &self.client_id_on_cosmos,
            self.eth_user_wallet.address,
            amount,
            denom,
            false,
        )
        .await
    }

    #[allow(clippy::future_not_send)]
    pub async fn send_eth_to_cosmos_transfer(&self, amount: u64, denom: &str) -> Result<()> {
        submit_eth_to_cosmos_transfer(
            &self.el_rpc_url,
            self.ics20_transfer,
            &self.eth_user_wallet.private_key,
            &self.client_id_on_eth,
            &self.cosmos_handle.user_wallets()[0].address,
            amount,
            denom,
        )
        .await
    }

    #[allow(clippy::future_not_send)]
    pub async fn query_ibcerc20_balance(&self, denom: &str, holder: Address) -> Result<U256> {
        query_erc20_balance(&self.el_rpc_url, self.ics20_transfer, denom, holder).await
    }

    #[allow(clippy::future_not_send)]
    pub async fn assert_eventual_eth_balance(
        &self,
        denom: &str,
        holder: Address,
        expected: u64,
        timeout: Duration,
    ) -> Result<()> {
        poll_eth_balance(&self.el_rpc_url, self.ics20_transfer, denom, holder, expected, timeout)
            .await
    }

    #[allow(clippy::future_not_send)]
    pub async fn query_cosmos_balance(&self, address: &str, denom: &str) -> Result<u64> {
        query_cosmos_bank_balance(&self.cosmos_handle, address, denom).await
    }

    #[allow(clippy::future_not_send)]
    pub async fn assert_eventual_cosmos_balance(
        &self,
        address: &str,
        denom: &str,
        expected: u64,
        timeout: Duration,
    ) -> Result<()> {
        poll_cosmos_balance(&self.cosmos_handle, address, denom, expected, timeout).await
    }
}

#[allow(clippy::future_not_send)]
async fn fund_eth_user_wallet(kurtosis: &KurtosisHandle) -> Result<AnvilWallet> {
    use alloy::network::TransactionBuilder;
    use alloy::primitives::U256;
    use alloy::rpc::types::TransactionRequest;

    let user_signer = alloy::signers::local::PrivateKeySigner::random();
    let user_address = user_signer.address();
    let user_private_key = hex::encode(user_signer.credential().to_bytes());

    let faucet_signer: alloy::signers::local::PrivateKeySigner = kurtosis
        .pre_funded_key
        .private_key
        .parse()
        .map_err(|e| eyre::eyre!("parsing faucet key: {e}"))?;

    let provider = ProviderBuilder::new()
        .wallet(alloy::network::EthereumWallet::from(faucet_signer))
        .connect_http(kurtosis.el_rpc_url.parse()?)
        .erased();

    let fund_amount = U256::from(10_000_000_000_000_000_000u128); // 10 ETH
    let tx = TransactionRequest::default()
        .with_to(user_address)
        .with_value(fund_amount);

    let pending = provider.send_transaction(tx).await?;
    pending.watch().await?;

    info!(
        address = %format!("{user_address:#x}"),
        "funded Kurtosis ETH user wallet with 10 ETH"
    );

    Ok(AnvilWallet {
        private_key: user_private_key,
        address: user_address,
    })
}
