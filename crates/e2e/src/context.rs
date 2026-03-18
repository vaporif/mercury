use std::time::Duration;

use eyre::{Context, Result, bail};
use ibc::core::host::types::identifiers::ClientId;
use ibc_proto::ibc::core::channel::v2::{MsgSendPacket, Payload};
use mercury_chain_traits::builders::ClientMessageBuilder;
use mercury_chain_traits::prelude::*;
use mercury_cosmos_counterparties::CosmosAdapter;
use mercury_cosmos_counterparties::chain::CosmosChain;
use mercury_cosmos_counterparties::config::{CosmosChainConfig, GasPrice};
use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
use mercury_cosmos_counterparties::types::{CosmosMessage, CosmosTxResponse};
use prost::Message;
use prost::Name as _;
use sha2::{Digest, Sha256};
use tracing::info;

use crate::bootstrap::cosmos_docker::CosmosDockerHandle;
use crate::bootstrap::traits::ChainHandle;

type Cosmos = CosmosAdapter<Secp256k1KeyPair>;

pub struct TestContext {
    pub handle_a: CosmosDockerHandle,
    pub handle_b: CosmosDockerHandle,
    pub cosmos_a: CosmosAdapter<Secp256k1KeyPair>,
    pub cosmos_b: CosmosAdapter<Secp256k1KeyPair>,
    pub client_id_a: ClientId,
    pub client_id_b: ClientId,
}

impl TestContext {
    pub async fn setup(handle_a: CosmosDockerHandle, handle_b: CosmosDockerHandle) -> Result<Self> {
        let cosmos_a = build_cosmos_chain(&handle_a).await?;
        let cosmos_b = build_cosmos_chain(&handle_b).await?;

        info!("creating IBC client on chain B for chain A");
        let payload_a = ClientPayloadBuilder::<Cosmos>::build_create_client_payload(&cosmos_a)
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let msg_create_b =
            ClientMessageBuilder::<CosmosChain<Secp256k1KeyPair>>::build_create_client_message(
                &cosmos_b, payload_a,
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let responses_b = cosmos_b
            .send_messages_with_responses(vec![msg_create_b])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let client_id_b = extract_client_id_from_events(&responses_b)?;
        info!(client_id = %client_id_b, "created client on chain B");

        info!("creating IBC client on chain A for chain B");
        let payload_b = ClientPayloadBuilder::<Cosmos>::build_create_client_payload(&cosmos_b)
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let msg_create_a =
            ClientMessageBuilder::<CosmosChain<Secp256k1KeyPair>>::build_create_client_message(
                &cosmos_a, payload_b,
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let responses_a = cosmos_a
            .send_messages_with_responses(vec![msg_create_a])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        let client_id_a = extract_client_id_from_events(&responses_a)?;
        info!(client_id = %client_id_a, "created client on chain A");

        info!("registering counterparties");
        let msg_register_a =
            ClientMessageBuilder::<CosmosChain<Secp256k1KeyPair>>::build_register_counterparty_message(
                &cosmos_a,
                &client_id_a,
                &client_id_b,
                mercury_core::MerklePrefix::ibc_default(),
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        cosmos_a
            .send_messages(vec![msg_register_a])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        let msg_register_b =
            ClientMessageBuilder::<CosmosChain<Secp256k1KeyPair>>::build_register_counterparty_message(
                &cosmos_b,
                &client_id_b,
                &client_id_a,
                mercury_core::MerklePrefix::ibc_default(),
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        cosmos_b
            .send_messages(vec![msg_register_b])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        info!("IBC v2 setup complete");

        Ok(Self {
            handle_a,
            handle_b,
            cosmos_a,
            cosmos_b,
            client_id_a,
            client_id_b,
        })
    }

    #[allow(clippy::missing_panics_doc)]
    pub async fn send_transfer_a_to_b(&self, amount: u64, denom: &str) -> Result<()> {
        self.send_transfer_a_to_b_with_timeout(amount, denom, 600)
            .await
    }

    #[allow(clippy::missing_panics_doc)]
    pub async fn send_transfer_a_to_b_with_timeout(
        &self,
        amount: u64,
        denom: &str,
        timeout_secs: u64,
    ) -> Result<()> {
        let user_a = &self.handle_a.user_wallets()[0];
        let user_b = &self.handle_b.user_wallets()[0];

        let user_chain = build_cosmos_chain_with_wallet(&self.handle_a, user_a).await?;

        let packet_data = serde_json::json!({
            "denom": denom,
            "amount": amount.to_string(),
            "sender": user_a.address,
            "receiver": user_b.address,
            "memo": "",
        });

        let timeout = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + timeout_secs;

        let msg = MsgSendPacket {
            source_client: self.client_id_a.to_string(),
            timeout_timestamp: timeout,
            payloads: vec![Payload {
                source_port: "transfer".to_string(),
                destination_port: "transfer".to_string(),
                version: "ics20-1".to_string(),
                encoding: "application/json".to_string(),
                value: serde_json::to_vec(&packet_data)?,
            }],
            signer: user_a.address.clone(),
        };

        let cosmos_msg = CosmosMessage {
            type_url: MsgSendPacket::type_url().into(),
            value: msg.encode_to_vec(),
        };

        let responses = user_chain
            .send_messages_with_responses(vec![cosmos_msg])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        info!(
            tx_hash = %responses.first().map_or("?", |r| r.hash.as_str()),
            timeout_secs,
            "IBC v2 transfer submitted on chain A"
        );
        Ok(())
    }

    #[allow(clippy::missing_panics_doc)]
    pub async fn send_transfer_b_to_a(&self, amount: u64, denom: &str) -> Result<()> {
        let user_b = &self.handle_b.user_wallets()[0];
        let user_a = &self.handle_a.user_wallets()[0];

        let user_chain = build_cosmos_chain_with_wallet(&self.handle_b, user_b).await?;

        let packet_data = serde_json::json!({
            "denom": denom,
            "amount": amount.to_string(),
            "sender": user_b.address,
            "receiver": user_a.address,
            "memo": "",
        });

        let timeout = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 600;

        let msg = MsgSendPacket {
            source_client: self.client_id_b.to_string(),
            timeout_timestamp: timeout,
            payloads: vec![Payload {
                source_port: "transfer".to_string(),
                destination_port: "transfer".to_string(),
                version: "ics20-1".to_string(),
                encoding: "application/json".to_string(),
                value: serde_json::to_vec(&packet_data)?,
            }],
            signer: user_b.address.clone(),
        };

        let cosmos_msg = CosmosMessage {
            type_url: MsgSendPacket::type_url().into(),
            value: msg.encode_to_vec(),
        };

        let responses = user_chain
            .send_messages_with_responses(vec![cosmos_msg])
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;

        info!(
            tx_hash = %responses.first().map_or("?", |r| r.hash.as_str()),
            "IBC v2 transfer submitted on chain B"
        );
        Ok(())
    }

    /// Returns `ibc/<SHA256_HEX>` — the bank module denomination.
    #[must_use]
    pub fn ibc_denom(port_id: &str, client_id: &str, base_denom: &str) -> String {
        let path = Self::denom_trace_path(port_id, client_id, base_denom);
        let hash = Sha256::digest(path.as_bytes());
        format!("ibc/{}", hex::encode_upper(hash))
    }

    /// Returns `<port_id>/<client_id>/<base_denom>` — the format used in
    /// `FungibleTokenPacketData` when sending IBC vouchers back via IBC v2.
    #[must_use]
    pub fn denom_trace_path(port_id: &str, client_id: &str, base_denom: &str) -> String {
        format!("{port_id}/{client_id}/{base_denom}")
    }

    #[allow(clippy::future_not_send)]
    pub async fn query_balance(
        handle: &CosmosDockerHandle,
        address: &str,
        denom: &str,
    ) -> Result<u64> {
        // Query all balances and filter — `--denom` returns exit code 1
        // when the denom doesn't exist on some simd versions.
        let cmd = format!(
            "simd query bank balances {address} \
             --home /root/.simapp --output json 2>/dev/null"
        );
        let output = handle.exec_cmd(&cmd).await?;
        let parsed: serde_json::Value =
            serde_json::from_str(output.trim()).unwrap_or(serde_json::Value::Null);
        let empty = Vec::new();
        let balances = parsed
            .get("balances")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);
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

    #[allow(clippy::future_not_send)]
    pub async fn assert_eventual_balance(
        &self,
        handle: &CosmosDockerHandle,
        address: &str,
        denom: &str,
        expected_amount: u64,
        timeout: Duration,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > timeout {
                bail!(
                    "timeout waiting for balance: address={address}, \
                     denom={denom}, expected={expected_amount}"
                );
            }

            match Self::query_balance(handle, address, denom).await {
                Ok(actual) if actual >= expected_amount => {
                    info!(
                        address = %address,
                        denom = %denom,
                        actual = actual,
                        expected = expected_amount,
                        "balance assertion passed"
                    );
                    return Ok(());
                }
                Ok(actual) => {
                    tracing::debug!(
                        actual = actual,
                        expected = expected_amount,
                        denom = %denom,
                        "balance not yet sufficient, polling..."
                    );
                }
                Err(e) => {
                    tracing::debug!(error = %e, "balance query failed, retrying...");
                }
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

fn build_signer_from_wallet(
    wallet: &crate::bootstrap::traits::Wallet,
    account_prefix: &str,
) -> Result<Secp256k1KeyPair> {
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

fn make_test_cosmos_config(handle: &CosmosDockerHandle, key_name: &str) -> CosmosChainConfig {
    CosmosChainConfig {
        chain_name: None,
        chain_id: handle.chain_id().to_string(),
        rpc_addr: handle.rpc_endpoint().to_string(),
        grpc_addr: handle.grpc_endpoint().to_string(),
        account_prefix: "cosmos".to_string(),
        key_name: key_name.to_string(),
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
        wasm_checksum: None,
        mock_proofs: false,
        rpc_timeout_secs: mercury_core::rpc_guard::default_timeout_secs(),
        rpc_rate_limit: mercury_core::rpc_guard::default_rate_limit(),
    }
}

async fn build_cosmos_chain_with_wallet(
    handle: &CosmosDockerHandle,
    wallet: &crate::bootstrap::traits::Wallet,
) -> Result<CosmosAdapter<Secp256k1KeyPair>> {
    let signer = build_signer_from_wallet(wallet, "cosmos")?;
    CosmosAdapter::new(make_test_cosmos_config(handle, "user"), signer)
        .await
        .map_err(|e| eyre::eyre!("{e}"))
}

async fn build_cosmos_chain(
    handle: &CosmosDockerHandle,
) -> Result<CosmosAdapter<Secp256k1KeyPair>> {
    let signer = build_signer_from_wallet(handle.relayer_wallet(), "cosmos")?;
    CosmosAdapter::new(make_test_cosmos_config(handle, "relayer"), signer)
        .await
        .map_err(|e| eyre::eyre!("{e}"))
}

/// Parse client ID from `MsgCreateClient` response events.
fn extract_client_id_from_events(responses: &[CosmosTxResponse]) -> Result<ClientId> {
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
    bail!("client_id not found in tx response events")
}
