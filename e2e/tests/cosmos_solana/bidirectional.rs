use std::sync::Arc;
use std::time::Duration;

use borsh::BorshSerialize;
use eyre::Result;
use ibc::core::host::types::identifiers::ClientId;
use ibc_proto::ibc::core::channel::v2::{MsgSendPacket, Payload};
use mercury_chain_cache::CachedChain;
use mercury_chain_traits::queries::{ChainStatusQuery, PacketStateQuery};
use mercury_chain_traits::types::PacketSequence;
use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
use mercury_cosmos_counterparties::wrapper::CosmosAdapter;
use mercury_relay::context::{RelayContext, RelayWorkerConfig};
use mercury_solana::accounts::{
    IbcApp, Ics07Tendermint, Ics26Router, OnChainClientState, deserialize_anchor_account,
    encode_anchor_instruction,
};
use mercury_solana::types::SolanaClientId;
use mercury_solana_counterparties::SolanaAdapter;
use prost::Message as _;
use prost::Name as _;
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::signer::Signer;
use solana_system_interface::program as system_program;
use tokio_util::sync::CancellationToken;
use tracing::info;

use mercury_e2e::bootstrap::traits::ChainHandle;

use super::bootstrap::{CosmosSolanaHarness, set_up_cosmos_solana};
use super::init_tracing;

type CosmosCached = CachedChain<CosmosAdapter<Secp256k1KeyPair>>;
type SolanaCached = CachedChain<SolanaAdapter>;

const TRANSFER_AMOUNT: u64 = 1_000_000;

#[derive(BorshSerialize)]
struct SendPacketMsg {
    source_client: String,
    source_port: String,
    dest_port: String,
    version: String,
    encoding: String,
    packet_data: Vec<u8>,
    timeout_timestamp: u64,
    sequence: u64,
}

#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Docker and solana-test-validator"]
async fn bidirectional_relay() -> Result<()> {
    init_tracing();

    let fixtures_dir = super::solana_fixtures_dir()?;
    let harness = set_up_cosmos_solana(&fixtures_dir).await?;
    info!(
        cosmos_client = %harness.cosmos_wasm_client_id,
        solana_client = %harness.solana_tendermint_client_id,
        "handshake complete",
    );

    let cosmos_client_id: ClientId = harness
        .cosmos_wasm_client_id
        .parse()
        .map_err(|e| eyre::eyre!("parse cosmos client id: {e}"))?;
    let solana_client_id = SolanaClientId(harness.solana_tendermint_client_id.clone());

    let cosmos_cached: CosmosCached =
        CachedChain::new(CosmosAdapter(harness.cosmos_chain.clone()));
    let solana_cached: SolanaCached = CachedChain::new(harness.solana_adapter.clone());

    let relay_cs = Arc::new(RelayContext {
        src_chain: cosmos_cached.clone(),
        dst_chain: solana_cached.clone(),
        src_client_id: cosmos_client_id.clone(),
        dst_client_id: solana_client_id.clone(),
    });
    let relay_sc = Arc::new(RelayContext {
        src_chain: solana_cached.clone(),
        dst_chain: cosmos_cached.clone(),
        src_client_id: solana_client_id.clone(),
        dst_client_id: cosmos_client_id.clone(),
    });

    let token = CancellationToken::new();
    let join_cs = tokio::spawn({
        let relay = Arc::clone(&relay_cs);
        let t = token.clone();
        async move { relay.run_with_token(t, RelayWorkerConfig::default()).await }
    });
    let join_sc = tokio::spawn({
        let relay = Arc::clone(&relay_sc);
        let t = token.clone();
        async move { relay.run_with_token(t, RelayWorkerConfig::default()).await }
    });

    let rpc = RpcClient::new_with_commitment(
        harness.solana_bootstrap.rpc_url.clone(),
        CommitmentConfig::confirmed(),
    );

    info!("sending ICS-20 packet from Cosmos to Solana");
    send_ics20_packet_from_cosmos(&harness).await?;

    poll_until(
        "packet receipt on Solana (Cosmos->Solana)",
        Duration::from_secs(90),
        || {
            let (pda, _) = Ics26Router::packet_receipt_pda(
                &harness.solana_tendermint_client_id,
                1,
                &harness.solana_bootstrap.program_ids.ics26,
            );
            rpc.get_account(&pda).is_ok()
        },
    )
    .await?;
    info!("Cosmos->Solana: packet receipt PDA exists on Solana");

    poll_until(
        "packet ack on Solana (Cosmos->Solana)",
        Duration::from_secs(30),
        || {
            let (pda, _) = Ics26Router::packet_ack_pda(
                &harness.solana_tendermint_client_id,
                1,
                &harness.solana_bootstrap.program_ids.ics26,
            );
            rpc.get_account(&pda).is_ok()
        },
    )
    .await?;
    info!("Cosmos->Solana: packet ack PDA exists on Solana");

    info!("sending packet from Solana to Cosmos via test_ibc_app");
    send_packet_from_solana(&harness, &rpc).await?;

    let (commitment_pda, _) = Ics26Router::packet_commitment_pda(
        &harness.solana_tendermint_client_id,
        1,
        &harness.solana_bootstrap.program_ids.ics26,
    );
    let commitment_account = rpc
        .get_account(&commitment_pda)
        .map_err(|e| eyre::eyre!("PacketCommitment PDA not found after send_packet: {e}"))?;
    assert!(
        !commitment_account.data.is_empty(),
        "PacketCommitment PDA should have data"
    );
    info!(?commitment_pda, "Solana->Cosmos: packet commitment PDA exists");

    poll_until_async(
        "packet receipt on Cosmos (Solana->Cosmos)",
        Duration::from_secs(90),
        || {
            let chain = harness.cosmos_chain.clone();
            let client_id = cosmos_client_id.clone();
            async move {
                let Ok(height) = chain.query_latest_height().await else {
                    return false;
                };
                match chain
                    .query_packet_receipt(&client_id, PacketSequence(1), &height)
                    .await
                {
                    Ok((receipt, _)) => receipt.is_some(),
                    Err(_) => false,
                }
            }
        },
    )
    .await?;
    info!("Solana->Cosmos: packet receipt exists on Cosmos");

    token.cancel();
    let _ = tokio::join!(join_cs, join_sc);
    info!("bidirectional_relay: all assertions passed");

    Ok(())
}

async fn send_ics20_packet_from_cosmos(harness: &CosmosSolanaHarness) -> Result<()> {
    let wallet = harness.cosmos_handle.relayer_wallet();

    let packet_data = serde_json::json!({
        "denom": "stake",
        "amount": TRANSFER_AMOUNT.to_string(),
        "sender": wallet.address,
        "receiver": "solana_receiver_placeholder",
        "memo": "",
    });

    #[allow(clippy::unwrap_used)]
    let timeout = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 30 * 60;

    let msg = MsgSendPacket {
        source_client: harness.cosmos_wasm_client_id.clone(),
        timeout_timestamp: timeout,
        payloads: vec![Payload {
            source_port: "transfer".to_string(),
            destination_port: "transfer".to_string(),
            version: "ics20-1".to_string(),
            encoding: "application/json".to_string(),
            value: serde_json::to_vec(&packet_data)?,
        }],
        signer: wallet.address.clone(),
    };

    let cosmos_msg = mercury_cosmos_counterparties::types::CosmosMessage {
        type_url: MsgSendPacket::type_url().into(),
        value: msg.encode_to_vec(),
    };

    let responses = harness
        .cosmos_chain
        .send_messages_with_responses(vec![cosmos_msg])
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;

    info!(
        tx_hash = %responses.first().map_or("?", |r| r.hash.as_str()),
        "ICS-20 packet submitted on Cosmos"
    );
    Ok(())
}

async fn send_packet_from_solana(
    harness: &CosmosSolanaHarness,
    rpc: &RpcClient,
) -> Result<()> {
    let payer = harness.solana_bootstrap.keypair.pubkey();
    let ics26 = harness.solana_bootstrap.program_ids.ics26;
    let ics07 = harness.solana_bootstrap.program_ids.ics07;
    let ibc_app_program = harness.solana_bootstrap.program_ids.ibc_app;
    let client_id = &harness.solana_tendermint_client_id;

    let (app_state_pda, _) = IbcApp::state_pda(&ibc_app_program);
    let (router_state_pda, _) = Ics26Router::router_state_pda(&ics26);
    let (ibc_app_pda, _) = Ics26Router::ibc_app_pda("transfer", &ics26);
    let (client_pda, _) = Ics26Router::client_pda(client_id, &ics26);
    let (commitment_pda, _) = Ics26Router::packet_commitment_pda(client_id, 1, &ics26);
    let (client_state_pda, _) = Ics07Tendermint::client_state_pda(&ics07);

    let cs_account = rpc
        .get_account(&client_state_pda)
        .map_err(|e| eyre::eyre!("ClientState PDA not found: {e}"))?;
    let cs: OnChainClientState = deserialize_anchor_account(&cs_account.data)?;
    let latest_height = cs.latest_height.revision_height;
    let (consensus_state_pda, _) = Ics07Tendermint::consensus_state_pda(latest_height, &ics07);

    #[allow(clippy::unwrap_used)]
    let timeout_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 30 * 60;

    let msg = SendPacketMsg {
        source_client: client_id.clone(),
        source_port: "transfer".to_string(),
        dest_port: "transfer".to_string(),
        version: "1".to_string(),
        encoding: "json".to_string(),
        packet_data: b"hello from solana".to_vec(),
        timeout_timestamp,
        // Sequence 1: first packet sent from this (Solana) source client.
        // Namespaced by source_client, so no collision with Cosmos->Solana seq 1.
        sequence: 1,
    };

    let data = encode_anchor_instruction("send_packet", &msg)?;

    let ix = Instruction {
        program_id: ibc_app_program,
        accounts: vec![
            AccountMeta::new(app_state_pda, false),
            AccountMeta::new(payer, true),
            AccountMeta::new_readonly(router_state_pda, false),
            AccountMeta::new_readonly(ibc_app_pda, false),
            AccountMeta::new(commitment_pda, false),
            AccountMeta::new_readonly(client_pda, false),
            AccountMeta::new_readonly(ics07, false),
            AccountMeta::new_readonly(client_state_pda, false),
            AccountMeta::new_readonly(consensus_state_pda, false),
            AccountMeta::new_readonly(ics26, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    };

    let solana_rpc = mercury_solana::rpc::SolanaRpcClient::new(
        &harness.solana_adapter.0.config,
    );
    let sig = mercury_solana::tx::send_transaction_skip_preflight(
        &solana_rpc,
        &harness.solana_bootstrap.keypair,
        vec![ix],
        None,
    )
    .await?;

    info!(%sig, "send_packet submitted on Solana via test_ibc_app");
    Ok(())
}

/// Poll a synchronous predicate until it returns `true` or a timeout elapses.
async fn poll_until(
    label: &str,
    timeout: Duration,
    mut f: impl FnMut() -> bool,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if f() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            eyre::bail!("timed out waiting for: {label}");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Poll an async predicate until it returns `true` or a timeout elapses.
async fn poll_until_async<F, Fut>(label: &str, timeout: Duration, mut f: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if f().await {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            eyre::bail!("timed out waiting for: {label}");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
