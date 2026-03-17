use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider as _, ProviderBuilder};
use alloy::sol_types::SolEvent;
use eyre::{Context as _, Result, bail};
use ibc::core::host::types::identifiers::ClientId;
use ibc_proto::ibc::core::channel::v2::{MsgSendPacket, Payload};
use mercury_chain_traits::builders::{ClientMessageBuilder, ClientPayloadBuilder};
use mercury_chain_traits::types::MessageSender;
use mercury_cosmos_counterparties::CosmosAdapter;
use mercury_cosmos_counterparties::chain::CosmosChain;
use mercury_cosmos_counterparties::config::{CosmosChainConfig, GasPrice};
use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
use mercury_cosmos_counterparties::types::{CosmosMessage, CosmosTxResponse};
use mercury_ethereum::config::{ClientPayloadMode, EthereumChainConfig};
use mercury_ethereum::contracts::{IBCERC20, ICS20Transfer, ICS26Router};
use mercury_ethereum::types::{EvmClientId, EvmTxResponse};
use mercury_ethereum_counterparties::EthereumAdapter;
use mercury_relay::context::{RelayContext, RelayWorkerConfig};
use prost::Message;
use prost::Name as _;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::bootstrap::anvil::{AnvilHandle, start_anvil};
use crate::bootstrap::cosmos_docker::{
    CosmosDockerBootstrap, CosmosDockerHandle, store_dummy_wasm_light_client,
};
use crate::bootstrap::traits::{ChainBootstrap, ChainHandle};
use crate::relayer::RelayHandle;

pub struct CosmosEthTestContext {
    pub cosmos_handle: CosmosDockerHandle,
    pub anvil_handle: AnvilHandle,
    pub cosmos_chain: CosmosAdapter<Secp256k1KeyPair>,
    pub eth_chain: EthereumAdapter,
    pub client_id_on_cosmos: ClientId,
    pub client_id_on_eth: EvmClientId,
}

impl CosmosEthTestContext {
    #[allow(clippy::future_not_send, clippy::too_many_lines)]
    pub async fn setup() -> Result<Self> {
        let cosmos_bootstrap = CosmosDockerBootstrap::new("mercury-cosmos");
        let cosmos_handle = cosmos_bootstrap.start().await?;

        let anvil_handle = start_anvil().await?;

        let wasm_checksum = store_dummy_wasm_light_client(&cosmos_handle).await?;

        let cosmos_chain = build_cosmos_chain(&cosmos_handle, Some(&wasm_checksum)).await?;

        info!("building SP1 programs and deriving vkeys");
        let elf_dir = crate::bootstrap::anvil::build_sp1_programs()?;
        let vkeys = crate::bootstrap::anvil::derive_sp1_vkeys(&elf_dir)?;

        let (client_state_abi, consensus_state_hash) =
            build_sp1_client_state(&cosmos_handle).await?;

        let sp1_light_client = crate::bootstrap::anvil::deploy_sp1_light_client(
            &anvil_handle.rpc_endpoint,
            &anvil_handle.relayer_wallet,
            anvil_handle.mock_verifier,
            &vkeys,
            &client_state_abi,
            consensus_state_hash,
        )?;

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
            light_client_address: Some(format!("{sp1_light_client:#x}")),
            client_payload_mode: ClientPayloadMode::Mock,
            sp1_prover: Some(mercury_ethereum::config::Sp1ProverConfig {
                elf_dir,
                zk_algorithm: mercury_ethereum::config::ZkAlgorithm::Groth16,
                prover_mode: mercury_ethereum::config::ProverMode::Mock,
                proof_timeout_secs: 120,
                max_concurrent_proofs: 4,
            }),
        };

        let eth_chain = EthereumAdapter::new(eth_config, eth_signer)
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        info!("creating IBC client on Cosmos for Ethereum");
        let eth_payload =
            ClientPayloadBuilder::<CosmosChain<Secp256k1KeyPair>>::build_create_client_payload(
                &eth_chain,
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let msg_create_cosmos =
            ClientMessageBuilder::<mercury_ethereum::chain::EthereumChain>::build_create_client_message(
                &cosmos_chain, eth_payload,
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let cosmos_responses = cosmos_chain
            .send_messages_with_responses(vec![msg_create_cosmos])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let client_id_on_cosmos = extract_cosmos_client_id(&cosmos_responses)?;
        info!(client_id = %client_id_on_cosmos, "created client on Cosmos");

        info!("creating IBC client on Ethereum for Cosmos");
        let cosmos_payload =
            ClientPayloadBuilder::<EthereumAdapter>::build_create_client_payload(&cosmos_chain)
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
        let msg_create_eth =
            ClientMessageBuilder::<CosmosChain<Secp256k1KeyPair>>::build_create_client_message(
                &eth_chain,
                cosmos_payload,
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let eth_responses = eth_chain
            .send_messages_with_responses(vec![msg_create_eth])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let client_id_on_eth = extract_evm_client_id(&eth_responses)?;
        info!(client_id = %client_id_on_eth, "created client on Ethereum");

        info!("registering counterparties");
        let msg_register_cosmos =
            ClientMessageBuilder::<mercury_ethereum::chain::EthereumChain>::build_register_counterparty_message(
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
            ClientMessageBuilder::<CosmosChain<Secp256k1KeyPair>>::build_register_counterparty_message(
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

    /// Start bidirectional Cosmos↔Ethereum relay workers in-process.
    pub fn start_relay_library(&self) -> Result<RelayHandle> {
        let token = CancellationToken::new();

        let relay_ce = Arc::new(RelayContext {
            src_chain: self.cosmos_chain.clone(),
            dst_chain: self.eth_chain.clone(),
            src_client_id: self.client_id_on_cosmos.clone(),
            dst_client_id: self.client_id_on_eth.clone(),
        });

        let relay_ec = Arc::new(RelayContext {
            src_chain: self.eth_chain.clone(),
            dst_chain: self.cosmos_chain.clone(),
            src_client_id: self.client_id_on_eth.clone(),
            dst_client_id: self.client_id_on_cosmos.clone(),
        });

        let token_ce = token.clone();
        let token_ec = token.clone();

        let join_ab = tokio::spawn(async move {
            relay_ce
                .run_with_token(token_ce, RelayWorkerConfig::default())
                .await
        });
        let join_ba = tokio::spawn(async move {
            relay_ec
                .run_with_token(token_ec, RelayWorkerConfig::default())
                .await
        });

        Ok(RelayHandle {
            cancel: token,
            join_ab,
            join_ba,
        })
    }

    /// Send an IBC v2 transfer from Cosmos user1 to Ethereum user1.
    #[allow(clippy::future_not_send)]
    pub async fn send_cosmos_to_eth_transfer(&self, amount: u64, denom: &str) -> Result<()> {
        use alloy::sol_types::SolValue;
        use ibc_eureka_solidity_types::msgs::IICS20TransferMsgs::FungibleTokenPacketData;

        let cosmos_user = &self.cosmos_handle.user_wallets()[0];
        let eth_user = &self.anvil_handle.user_wallets[0];

        let user_chain =
            build_cosmos_chain_with_user(&self.cosmos_handle, cosmos_user, None).await?;

        let packet_data = FungibleTokenPacketData {
            denom: denom.to_string(),
            sender: cosmos_user.address.clone(),
            receiver: format!("{:#x}", eth_user.address),
            amount: U256::from(amount),
            memo: String::new(),
        };

        let timeout = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
            + 600;

        let msg = MsgSendPacket {
            source_client: self.client_id_on_cosmos.to_string(),
            timeout_timestamp: timeout,
            payloads: vec![Payload {
                source_port: "transfer".to_string(),
                destination_port: "transfer".to_string(),
                version: "ics20-1".to_string(),
                encoding: "application/x-solidity-abi".to_string(),
                value: packet_data.abi_encode(),
            }],
            signer: cosmos_user.address.clone(),
        };

        let cosmos_msg = CosmosMessage {
            type_url: MsgSendPacket::type_url(),
            value: msg.encode_to_vec(),
        };

        let responses = user_chain
            .send_messages_with_responses(vec![cosmos_msg])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        info!(
            tx_hash = %responses.first().map_or("?", |r| r.hash.as_str()),
            %amount,
            %denom,
            "IBC v2 transfer submitted from Cosmos to Ethereum"
        );

        Ok(())
    }

    /// Send an IBC v2 transfer from Ethereum user1 back to Cosmos user1.
    #[allow(clippy::future_not_send)]
    pub async fn send_eth_to_cosmos_transfer(&self, amount: u64, denom: &str) -> Result<()> {
        use mercury_ethereum::contracts::ics20_transfer::IICS20TransferMsgs;

        let eth_user = &self.anvil_handle.user_wallets[0];
        let cosmos_user = &self.cosmos_handle.user_wallets()[0];

        let eth_signer: alloy::signers::local::PrivateKeySigner = eth_user
            .private_key
            .parse()
            .map_err(|e| eyre::eyre!("parsing eth user private key: {e}"))?;

        let provider = ProviderBuilder::new()
            .wallet(alloy::network::EthereumWallet::from(eth_signer))
            .connect_http(self.anvil_handle.rpc_endpoint.parse()?)
            .erased();

        let ics20 = ICS20Transfer::new(self.anvil_handle.ics20_transfer, &provider);
        let erc20_addr = Address::from(ics20.ibcERC20Contract(denom.to_string()).call().await?.0);

        if erc20_addr == Address::ZERO {
            bail!("IBCERC20 not found for denom: {denom}");
        }

        let erc20 = IBCERC20::new(erc20_addr, &provider);
        let approve_tx = erc20.approve(self.anvil_handle.ics20_transfer, U256::from(amount));
        let pending = approve_tx.send().await?;
        pending.watch().await?;
        info!(%denom, %amount, "approved ICS20Transfer to spend IBCERC20");

        let timeout = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
            + 600;

        let msg = IICS20TransferMsgs::SendTransferMsg {
            denom: erc20_addr,
            amount: U256::from(amount),
            receiver: cosmos_user.address.clone(),
            sourceClient: self.client_id_on_eth.0.clone(),
            destPort: "transfer".to_string(),
            timeoutTimestamp: timeout,
            memo: String::new(),
        };

        let tx = ics20.sendTransfer(msg).send().await?;
        let receipt = tx.watch().await?;

        info!(
            tx_hash = %receipt,
            %amount,
            %denom,
            "IBC v2 transfer submitted from Ethereum to Cosmos"
        );

        Ok(())
    }

    /// Query the IBCERC20 balance on Ethereum for a received IBC denom.
    ///
    /// `denom` is the full IBC denom path, e.g. `transfer/{client_id_on_eth}/stake`.
    #[allow(clippy::future_not_send)]
    pub async fn query_ibcerc20_balance(&self, denom: &str, holder: Address) -> Result<U256> {
        let provider = ProviderBuilder::new()
            .connect_http(self.anvil_handle.rpc_endpoint.parse()?)
            .erased();

        let ics20 = ICS20Transfer::new(self.anvil_handle.ics20_transfer, &provider);
        let erc20_addr = Address::from(ics20.ibcERC20Contract(denom.to_string()).call().await?.0);

        if erc20_addr == Address::ZERO {
            return Ok(U256::ZERO);
        }

        let erc20 = IBCERC20::new(erc20_addr, &provider);
        Ok(erc20.balanceOf(holder).call().await?)
    }

    /// Poll Ethereum for expected IBCERC20 balance, with timeout.
    #[allow(clippy::future_not_send)]
    pub async fn assert_eventual_eth_balance(
        &self,
        denom: &str,
        holder: Address,
        expected: u64,
        timeout: Duration,
    ) -> Result<()> {
        let expected_u256 = U256::from(expected);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                bail!(
                    "timeout waiting for IBCERC20 balance: denom={denom}, \
                     holder={holder:#x}, expected={expected}"
                );
            }

            match self.query_ibcerc20_balance(denom, holder).await {
                Ok(actual) if actual >= expected_u256 => {
                    info!(
                        %denom,
                        holder = %format!("{holder:#x}"),
                        actual = %actual,
                        expected = %expected,
                        "IBCERC20 balance assertion passed"
                    );
                    return Ok(());
                }
                Ok(actual) => {
                    tracing::debug!(
                        actual = %actual,
                        expected = %expected,
                        %denom,
                        "IBCERC20 balance not yet sufficient, polling..."
                    );
                }
                Err(e) => {
                    tracing::debug!(error = %e, "IBCERC20 balance query failed, retrying...");
                }
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Query bank balance on Cosmos for a given address and denom.
    #[allow(clippy::future_not_send)]
    pub async fn query_cosmos_balance(&self, address: &str, denom: &str) -> Result<u64> {
        let cmd = format!(
            "simd query bank balances {address} \
             --home /root/.simapp --output json 2>/dev/null"
        );
        let output = self.cosmos_handle.exec_cmd(&cmd).await?;
        let parsed: serde_json::Value =
            serde_json::from_str(output.trim()).wrap_err("parsing cosmos bank balances output")?;
        let balances = parsed
            .get("balances")
            .and_then(|v| v.as_array())
            .map_or(&[] as &[_], Vec::as_slice);
        for bal in balances {
            let d = bal.get("denom").and_then(|v| v.as_str()).unwrap_or("");
            if d == denom {
                let amount_str = bal.get("amount").and_then(|v| v.as_str()).unwrap_or("0");
                return amount_str
                    .parse::<u64>()
                    .map_err(|e| eyre::eyre!("parse balance amount: {e}"));
            }
        }
        Ok(0)
    }

    /// Poll Cosmos for expected bank balance, with timeout.
    #[allow(clippy::future_not_send)]
    pub async fn assert_eventual_cosmos_balance(
        &self,
        address: &str,
        denom: &str,
        expected: u64,
        timeout: Duration,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > timeout {
                bail!(
                    "timeout waiting for Cosmos balance: address={address}, \
                     denom={denom}, expected={expected}"
                );
            }

            match self.query_cosmos_balance(address, denom).await {
                Ok(actual) if actual >= expected => {
                    info!(
                        %address,
                        %denom,
                        actual = actual,
                        expected = expected,
                        "Cosmos balance assertion passed"
                    );
                    return Ok(());
                }
                Ok(actual) => {
                    tracing::debug!(
                        actual = actual,
                        expected = expected,
                        %denom,
                        "Cosmos balance not yet sufficient, polling..."
                    );
                }
                Err(e) => {
                    tracing::debug!(error = %e, "Cosmos balance query failed, retrying...");
                }
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

/// Build the Solidity-encoded client state and consensus state hash for
/// `SP1ICS07Tendermint` deployment, using real Cosmos chain state.
async fn build_sp1_client_state(
    cosmos_handle: &CosmosDockerHandle,
) -> Result<(Vec<u8>, alloy::primitives::B256)> {
    use alloy::primitives::{B256, keccak256};
    use alloy::sol_types::SolValue;
    use ibc_eureka_solidity_types::msgs::IICS02ClientMsgs::Height as SolHeight;
    use ibc_eureka_solidity_types::msgs::IICS07TendermintMsgs::{
        ClientState as SolClientState, ConsensusState as SolConsensusState, SupportedZkAlgorithm,
        TrustThreshold,
    };
    use tendermint_rpc::{Client, HttpClient};

    let client =
        HttpClient::new(cosmos_handle.rpc_endpoint()).wrap_err("creating tendermint RPC client")?;

    let latest_block = client
        .latest_block()
        .await
        .wrap_err("querying latest Cosmos block")?;

    let header = &latest_block.block.header;
    let chain_id: ibc::core::host::types::identifiers::ChainId = header
        .chain_id
        .to_string()
        .parse()
        .map_err(|e| eyre::eyre!("parsing chain ID: {e}"))?;

    let height = header.height.value();
    let revision_number = chain_id.revision_number();

    // Match eureka defaults: trust_threshold = 1/3, trusting_period = 2/3 * unbonding
    // For e2e with short unbonding, use reasonable defaults.
    let unbonding_period: u32 = 1_209_600; // 14 days in seconds
    let trusting_period: u32 = 2 * (unbonding_period / 3);

    let client_state = SolClientState {
        chainId: chain_id.to_string(),
        trustLevel: TrustThreshold {
            numerator: 1,
            denominator: 3,
        },
        latestHeight: SolHeight {
            revisionNumber: revision_number,
            revisionHeight: height,
        },
        isFrozen: false,
        zkAlgorithm: SupportedZkAlgorithm::Groth16,
        unbondingPeriod: unbonding_period,
        trustingPeriod: trusting_period,
    };

    #[allow(clippy::cast_sign_loss)]
    let ts_nanos = header.time.unix_timestamp_nanos() as u128;

    let consensus_state = SolConsensusState {
        timestamp: ts_nanos,
        root: B256::from_slice(header.app_hash.as_bytes()),
        nextValidatorsHash: B256::from_slice(header.next_validators_hash.as_bytes()),
    };

    let client_state_abi = client_state.abi_encode();
    let consensus_state_hash = keccak256(consensus_state.abi_encode());

    Ok((client_state_abi, consensus_state_hash))
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
        mock_proofs: true,
    }
}

async fn build_cosmos_chain(
    handle: &CosmosDockerHandle,
    wasm_checksum: Option<&str>,
) -> Result<CosmosAdapter<Secp256k1KeyPair>> {
    build_cosmos_chain_with_user(handle, handle.relayer_wallet(), wasm_checksum).await
}

async fn build_cosmos_chain_with_user(
    handle: &CosmosDockerHandle,
    wallet: &crate::bootstrap::traits::Wallet,
    wasm_checksum: Option<&str>,
) -> Result<CosmosAdapter<Secp256k1KeyPair>> {
    let secret_bytes =
        hex::decode(&wallet.secret_key_hex).wrap_err("decoding wallet secret key hex")?;
    let secret_arr: [u8; 32] = secret_bytes
        .try_into()
        .map_err(|_| eyre::eyre!("secret key must be 32 bytes"))?;
    let secret_key = secp256k1::SecretKey::from_byte_array(secret_arr)
        .map_err(|e| eyre::eyre!("invalid secret key: {e}"))?;
    let signer = Secp256k1KeyPair::from_secret_key(secret_key, "cosmos");

    CosmosAdapter::new(make_cosmos_config(handle, wasm_checksum), signer)
        .await
        .map_err(|e| eyre::eyre!("{e}"))
}
