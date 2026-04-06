use std::path::Path;
use std::time::Duration;

use eyre::{Result, WrapErr};
use mercury_chain_traits::builders::{ClientMessageBuilder, ClientPayloadBuilder};
use mercury_chain_traits::types::MessageSender;
use mercury_cosmos_counterparties::builders::CosmosCreateClientPayload;
use mercury_cosmos_counterparties::chain::CosmosChain;
use mercury_cosmos_counterparties::config::{CosmosChainConfig, GasPrice};
use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
use mercury_e2e::bootstrap::cosmos_docker::{
    CosmosDockerBootstrap, CosmosDockerHandle, create_dummy_wasm_client,
    store_dummy_wasm_light_client,
};
use mercury_e2e::bootstrap::solana::SolanaBootstrap;
use mercury_e2e::bootstrap::traits::{ChainBootstrap, ChainHandle, Wallet};
use mercury_solana::config::SolanaChainConfig;
use mercury_solana_counterparties::SolanaAdapter;
use tracing::info;

pub struct CosmosSolanaHarness {
    pub cosmos_handle: CosmosDockerHandle,
    pub cosmos_chain: CosmosChain<Secp256k1KeyPair>,
    pub solana_bootstrap: SolanaBootstrap,
    pub solana_adapter: SolanaAdapter,
    pub cosmos_wasm_client_id: String,
    pub solana_tendermint_client_id: String,
}

#[allow(clippy::future_not_send)]
pub async fn set_up_cosmos_solana(fixtures_dir: &Path) -> Result<CosmosSolanaHarness> {
    let cosmos_bootstrap = CosmosDockerBootstrap::new("mercury-cosmos-solana");
    let cosmos_handle = cosmos_bootstrap.start().await?;
    let solana_bootstrap = SolanaBootstrap::start(fixtures_dir)?;

    let cosmos_chain = build_cosmos_chain(&cosmos_handle).await?;
    let solana_adapter = build_solana_adapter(&solana_bootstrap)?;

    let wasm_checksum = store_dummy_wasm_light_client(&cosmos_handle).await?;
    let cosmos_wasm_client_id = create_dummy_wasm_client(&cosmos_handle, &wasm_checksum).await?;

    let mut payload: CosmosCreateClientPayload =
        <CosmosChain<Secp256k1KeyPair> as ClientPayloadBuilder<SolanaAdapter>>::build_create_client_payload(&cosmos_chain)
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
    payload.counterparty_client_id = Some(cosmos_wasm_client_id.clone());
    payload.counterparty_merkle_prefix = Some(cosmos_chain.config.merkle_prefix.clone());
    let create_msg =
        <SolanaAdapter as ClientMessageBuilder<CosmosChain<Secp256k1KeyPair>>>::build_create_client_message(
            &solana_adapter, payload,
        )
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;
    solana_adapter
        .send_messages(vec![create_msg])
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;

    let solana_tendermint_client_id =
        mercury_solana_counterparties::DEFAULT_TENDERMINT_CLIENT_ID.to_string();

    register_counterparty_on_cosmos(
        &cosmos_chain,
        &cosmos_wasm_client_id,
        &solana_tendermint_client_id,
    )
    .await?;

    Ok(CosmosSolanaHarness {
        cosmos_handle,
        cosmos_chain,
        solana_bootstrap,
        solana_adapter,
        cosmos_wasm_client_id,
        solana_tendermint_client_id,
    })
}

async fn build_cosmos_chain(handle: &CosmosDockerHandle) -> Result<CosmosChain<Secp256k1KeyPair>> {
    let wallet = handle.relayer_wallet();
    let signer = build_signer_from_wallet(wallet, "cosmos")?;
    let config = CosmosChainConfig {
        chain_name: None,
        chain_id: handle.chain_id().to_string(),
        rpc_addr: handle.rpc_endpoint().to_string(),
        ws_addr: None,
        grpc_addr: handle.grpc_endpoint().to_string(),
        account_prefix: "cosmos".to_string(),
        key_name: "relayer".to_string(),
        key_file: std::path::PathBuf::new(),
        gas_price: GasPrice {
            amount: 0.0,
            denom: "stake".to_string(),
        },
        block_time: Duration::from_secs(1),
        max_msg_num: 30,
        trusting_period: None,
        unbonding_period: None,
        max_clock_drift: None,
        gas_multiplier: None,
        max_gas: None,
        default_gas: None,
        fee_granter: None,
        dynamic_gas_price: None,
        max_tx_size: None,
        tx_size_gas_per_byte: None,
        wasm_checksum: None,
        mock_proofs: false,
        rpc_timeout_secs: mercury_core::rpc_guard::default_timeout_secs(),
        rpc_rate_limit: mercury_core::rpc_guard::default_rate_limit(),
        merkle_prefix: mercury_core::MerklePrefix::ibc_default(),
    };
    CosmosChain::new(config, signer)
        .await
        .map_err(|e| eyre::eyre!("{e}"))
}

fn build_solana_adapter(bootstrap: &SolanaBootstrap) -> Result<SolanaAdapter> {
    let keypair_path = tempfile::NamedTempFile::new()?.into_temp_path();
    let kp_bytes: Vec<u8> = bootstrap.keypair.to_bytes().to_vec();
    std::fs::write(&keypair_path, serde_json::to_vec(&kp_bytes)?)?;

    let config = SolanaChainConfig {
        rpc_addr: bootstrap.rpc_url.clone(),
        ws_addr: None,
        program_id: bootstrap.program_ids.ics26.to_string(),
        ics07_program_id: Some(bootstrap.program_ids.ics07.to_string()),
        keypair_path: keypair_path.to_path_buf(),
        block_time: Duration::from_millis(400),
        rpc_timeout_secs: mercury_core::rpc_guard::default_timeout_secs(),
        rpc_rate_limit: mercury_core::rpc_guard::default_rate_limit(),
        alt_address: None,
        skip_pre_verify_threshold: None,
    };
    SolanaAdapter::new(config).map_err(|e| eyre::eyre!("{e}"))
}

async fn register_counterparty_on_cosmos(
    cosmos: &CosmosChain<Secp256k1KeyPair>,
    cosmos_wasm_client_id: &str,
    solana_tendermint_client_id: &str,
) -> Result<()> {
    use ibc::core::host::types::identifiers::ClientId;

    let local_client_id: ClientId = cosmos_wasm_client_id
        .parse()
        .map_err(|e| eyre::eyre!("parse cosmos client id: {e}"))?;
    let counterparty_client_id: ClientId = solana_tendermint_client_id
        .parse()
        .map_err(|e| eyre::eyre!("parse solana client id: {e}"))?;

    let msg = <CosmosChain<Secp256k1KeyPair> as ClientMessageBuilder<
        CosmosChain<Secp256k1KeyPair>,
    >>::build_register_counterparty_message(
        cosmos,
        &local_client_id,
        &counterparty_client_id,
        mercury_core::MerklePrefix::ibc_default(),
    )
    .await
    .map_err(|e| eyre::eyre!("{e}"))?;

    cosmos
        .send_messages(vec![msg])
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;

    info!(
        cosmos_client = %cosmos_wasm_client_id,
        solana_client = %solana_tendermint_client_id,
        "registered counterparty on Cosmos"
    );
    Ok(())
}

fn build_signer_from_wallet(wallet: &Wallet, account_prefix: &str) -> Result<Secp256k1KeyPair> {
    let secret_bytes =
        hex::decode(&wallet.secret_key_hex).wrap_err("decoding wallet secret key hex")?;
    let secret_arr: [u8; 32] = secret_bytes
        .try_into()
        .map_err(|_| eyre::eyre!("secret key must be 32 bytes"))?;
    let secret_key = secp256k1::SecretKey::from_byte_array(secret_arr)
        .map_err(|e| eyre::eyre!("invalid secret key: {e}"))?;
    Ok(Secp256k1KeyPair::from_secret_key(
        secret_key,
        account_prefix,
    ))
}
