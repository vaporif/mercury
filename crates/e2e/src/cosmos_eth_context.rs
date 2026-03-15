use std::path::PathBuf;
use std::time::Duration;

use alloy::sol_types::SolEvent;
use eyre::{Context as _, Result, bail};
use ibc::core::host::types::identifiers::ClientId;
use mercury_chain_traits::builders::{ClientMessageBuilder, ClientPayloadBuilder};
use mercury_chain_traits::types::MessageSender;
use mercury_cosmos_bridges::CosmosChain;
use mercury_cosmos_bridges::chain::CosmosChainInner;
use mercury_cosmos_bridges::config::{CosmosChainConfig, GasPrice};
use mercury_cosmos_bridges::keys::Secp256k1KeyPair;
use mercury_cosmos_bridges::types::CosmosTxResponse;
use mercury_ethereum::config::{ClientPayloadMode, EthereumChainConfig};
use mercury_ethereum::contracts::ICS26Router;
use mercury_ethereum::types::{EvmClientId, EvmTxResponse};
use mercury_ethereum_bridges::EthereumChain;
use tracing::info;

use crate::bootstrap::anvil::{AnvilHandle, start_anvil};
use crate::bootstrap::cosmos_docker::{
    CosmosDockerBootstrap, CosmosDockerHandle, store_dummy_wasm_light_client,
};
use crate::bootstrap::traits::{ChainBootstrap, ChainHandle};

pub struct CosmosEthTestContext {
    pub cosmos_handle: CosmosDockerHandle,
    pub anvil_handle: AnvilHandle,
    pub cosmos_chain: CosmosChain<Secp256k1KeyPair>,
    pub eth_chain: EthereumChain,
    pub client_id_on_cosmos: ClientId,
    pub client_id_on_eth: EvmClientId,
}

impl CosmosEthTestContext {
    #[allow(clippy::future_not_send)]
    pub async fn setup() -> Result<Self> {
        // 1. Start Cosmos
        let cosmos_bootstrap = CosmosDockerBootstrap::new("mercury-cosmos");
        let cosmos_handle = cosmos_bootstrap.start().await?;

        // 2. Start Anvil + deploy contracts
        let anvil_handle = start_anvil().await?;

        // 3. Store dummy Wasm light client on Cosmos
        let wasm_checksum = store_dummy_wasm_light_client(&cosmos_handle).await?;

        // 4. Build CosmosChain
        let cosmos_chain = build_cosmos_chain(&cosmos_handle, Some(&wasm_checksum)).await?;

        // 5. Build EthereumChain
        let eth_signer: alloy::signers::local::PrivateKeySigner = anvil_handle
            .relayer_wallet
            .private_key
            .parse()
            .map_err(|e| eyre::eyre!("parsing anvil private key: {e}"))?;

        let eth_config = EthereumChainConfig {
            chain_id: anvil_handle.chain_id,
            rpc_addr: anvil_handle.rpc_endpoint.clone(),
            ics26_router: format!("{:#x}", anvil_handle.ics26_router),
            key_file: PathBuf::new(),
            block_time_secs: 1,
            deployment_block: 0,
            light_client_address: Some(format!("{:#x}", anvil_handle.mock_verifier)),
            client_payload_mode: ClientPayloadMode::Mock,
            sp1_prover: None,
        };

        let eth_chain = EthereumChain::new(eth_config, eth_signer)
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        // 6. Create client on Cosmos for Ethereum (Wasm-wrapped beacon client)
        info!("creating IBC client on Cosmos for Ethereum");
        let eth_payload = ClientPayloadBuilder::<CosmosChainInner<Secp256k1KeyPair>>::build_create_client_payload(&eth_chain)
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let msg_create_cosmos =
            ClientMessageBuilder::<mercury_ethereum::chain::EthereumChainInner>::build_create_client_message(
                &cosmos_chain, eth_payload,
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let cosmos_responses = cosmos_chain
            .send_messages(vec![msg_create_cosmos])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let client_id_on_cosmos = extract_cosmos_client_id(&cosmos_responses)?;
        info!(client_id = %client_id_on_cosmos, "created client on Cosmos");

        // 7. Create client on Ethereum for Cosmos (register mock verifier on ICS26Router)
        info!("creating IBC client on Ethereum for Cosmos");
        let cosmos_payload =
            ClientPayloadBuilder::<EthereumChain>::build_create_client_payload(&cosmos_chain)
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
        let msg_create_eth =
            ClientMessageBuilder::<CosmosChainInner<Secp256k1KeyPair>>::build_create_client_message(
                &eth_chain, cosmos_payload,
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let eth_responses = eth_chain
            .send_messages(vec![msg_create_eth])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let client_id_on_eth = extract_evm_client_id(&eth_responses)?;
        info!(client_id = %client_id_on_eth, "created client on Ethereum");

        // 8. Register counterparties
        info!("registering counterparties");
        let msg_register_cosmos =
            ClientMessageBuilder::<mercury_ethereum::chain::EthereumChainInner>::build_register_counterparty_message(
                &cosmos_chain,
                &client_id_on_cosmos,
                &client_id_on_eth,
                mercury_core::MerklePrefix::ibc_default(),
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        cosmos_chain
            .send_messages(vec![msg_register_cosmos])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        let msg_register_eth =
            ClientMessageBuilder::<CosmosChainInner<Secp256k1KeyPair>>::build_register_counterparty_message(
                &eth_chain,
                &client_id_on_eth,
                &client_id_on_cosmos,
                mercury_core::MerklePrefix::ibc_default(),
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        eth_chain
            .send_messages(vec![msg_register_eth])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        info!("Cosmos-Ethereum IBC v2 setup complete");

        Ok(Self {
            cosmos_handle,
            anvil_handle,
            cosmos_chain,
            eth_chain,
            client_id_on_cosmos,
            client_id_on_eth,
        })
    }
}

fn extract_cosmos_client_id(responses: &[CosmosTxResponse]) -> Result<ClientId> {
    for response in responses {
        for event in &response.events {
            for (key, value) in &event.attributes {
                if key == "client_id" {
                    return value
                        .parse()
                        .map_err(|e| eyre::eyre!("parse client_id: {e}"));
                }
            }
        }
    }
    bail!("client_id not found in Cosmos tx response events")
}

fn extract_evm_client_id(responses: &[EvmTxResponse]) -> Result<EvmClientId> {
    for response in responses {
        for log in &response.logs {
            if let Ok(event) = ICS26Router::ICS02ClientAdded::decode_log_data(
                &alloy::primitives::LogData::new_unchecked(
                    log.topics.clone(),
                    log.data.clone().into(),
                ),
            ) {
                return Ok(EvmClientId(event.clientId));
            }
        }
    }
    bail!("ICS02ClientAdded event not found in EVM tx response logs")
}

fn make_cosmos_config(
    handle: &CosmosDockerHandle,
    wasm_checksum: Option<&str>,
) -> CosmosChainConfig {
    CosmosChainConfig {
        chain_id: handle.chain_id().to_string(),
        rpc_addr: handle.rpc_endpoint().to_string(),
        grpc_addr: handle.grpc_endpoint().to_string(),
        account_prefix: "cosmos".to_string(),
        key_name: "relayer".to_string(),
        key_file: PathBuf::new(),
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
        wasm_checksum: wasm_checksum.map(String::from),
    }
}

async fn build_cosmos_chain(
    handle: &CosmosDockerHandle,
    wasm_checksum: Option<&str>,
) -> Result<CosmosChain<Secp256k1KeyPair>> {
    let wallet = handle.relayer_wallet();
    let secret_bytes =
        hex::decode(&wallet.secret_key_hex).wrap_err("decoding wallet secret key hex")?;
    let secret_arr: [u8; 32] = secret_bytes
        .try_into()
        .map_err(|_| eyre::eyre!("secret key must be 32 bytes"))?;
    let secret_key = secp256k1::SecretKey::from_byte_array(secret_arr)
        .map_err(|e| eyre::eyre!("invalid secret key: {e}"))?;
    let signer = Secp256k1KeyPair::from_secret_key(secret_key, "cosmos");

    CosmosChain::new(make_cosmos_config(handle, wasm_checksum), signer)
        .await
        .map_err(|e| eyre::eyre!("{e}"))
}
