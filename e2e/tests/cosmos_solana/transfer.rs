use eyre::Result;
use ibc_proto::ibc::core::channel::v2::{MsgSendPacket, Payload};
use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, PacketMessageBuilder,
};
use mercury_chain_traits::events::PacketEvents;
use mercury_chain_traits::queries::{ChainStatusQuery, PacketStateQuery};
use mercury_chain_traits::types::{ChainTypes, MessageSender};
use mercury_cosmos_counterparties::chain::CosmosChain;
use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
use mercury_cosmos_counterparties::types::{CosmosPacket, SendPacketEvent};
use mercury_solana::accounts::{
    Ics07Tendermint, Ics26Router, OnChainClientState, deserialize_anchor_account,
};
use mercury_solana::types::SolanaClientState;
use mercury_solana_counterparties::SolanaAdapter;
use prost::Message as _;
use prost::Name as _;
use sha2::{Digest, Sha256};
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use tracing::info;

use mercury_e2e::bootstrap::traits::ChainHandle;

use super::bootstrap::{CosmosSolanaHarness, set_up_cosmos_solana};
use super::init_tracing;

const TRANSFER_AMOUNT: u64 = 1_000_000;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Docker and solana-test-validator"]
async fn cosmos_to_solana_transfer() -> Result<()> {
    init_tracing();

    let fixtures_dir = std::path::PathBuf::from(
        std::env::var("SOLANA_PROGRAMS_DIR")
            .map_err(|_| eyre::eyre!("SOLANA_PROGRAMS_DIR env var must be set"))?,
    );

    let harness = set_up_cosmos_solana(&fixtures_dir).await?;
    info!(
        cosmos_client = %harness.cosmos_wasm_client_id,
        solana_client = %harness.solana_tendermint_client_id,
        "handshake complete",
    );

    let rpc = RpcClient::new_with_commitment(
        harness.solana_bootstrap.rpc_url.clone(),
        CommitmentConfig::confirmed(),
    );
    assert_client_state_exists(&rpc, &harness)?;
    info!("Solana-side Tendermint client verified");

    let (tx_responses, packet_height) = send_ics20_packet(&harness).await?;
    info!(%packet_height, "ICS20 packet sent");

    let packet = extract_send_packet(&tx_responses)?;
    info!(sequence = %packet.sequence.0, "SendPacket event decoded");

    // Cosmos commits state at height H into block H+1's app_hash, so we need
    // to update the client to at least packet_height+1 for the proof to be
    // verifiable against the stored consensus state root.
    let target_height = wait_for_height_past(&harness.cosmos_chain, packet_height).await?;
    info!(%target_height, %packet_height, "cosmos advanced past packet height");

    let trusted_revision_height = get_client_latest_height(&rpc, &harness)?;
    info!(
        trusted_revision_height,
        "using on-chain client latest_height as trusted height"
    );
    let trusted_height = tendermint::block::Height::try_from(trusted_revision_height)
        .map_err(|e| eyre::eyre!("height conversion: {e}"))?;

    let update_payload =
        <CosmosChain<Secp256k1KeyPair> as ClientPayloadBuilder<SolanaAdapter>>::build_update_client_payload(
            &harness.cosmos_chain,
            &trusted_height,
            &target_height,
            &SolanaClientState(vec![]),
        )
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;

    let update_output =
        <SolanaAdapter as ClientMessageBuilder<CosmosChain<Secp256k1KeyPair>>>::build_update_client_message(
            &harness.solana_adapter,
            &mercury_solana::types::SolanaClientId(harness.solana_tendermint_client_id.clone()),
            update_payload,
        )
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;

    harness
        .solana_adapter
        .send_messages(update_output.messages)
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;
    info!("update-client submitted to Solana");

    assert_consensus_state_stored(&rpc, &harness, target_height.value())?;

    let cosmos_client_id: ibc::core::host::types::identifiers::ClientId = harness
        .cosmos_wasm_client_id
        .parse()
        .map_err(|e| eyre::eyre!("parse client id: {e}"))?;

    info!(
        %target_height,
        sequence = %packet.sequence.0,
        client_id = %cosmos_client_id,
        "querying packet commitment proof"
    );
    let (commitment, proof) = harness
        .cosmos_chain
        .query_packet_commitment(&cosmos_client_id, packet.sequence, &target_height)
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;
    info!(
        proof_len = proof.proof_bytes.len(),
        commitment_present = commitment.is_some(),
        commitment_hex = %commitment.as_ref().map(|c| hex::encode(&c.0)).unwrap_or_default(),
        "packet commitment proof obtained"
    );

    // Log packet fields that affect on-chain commitment recomputation
    info!(
        source_client = %packet.source_client_id.0,
        dest_client = %packet.dest_client_id.0,
        sequence = packet.sequence.0,
        timeout_timestamp = packet.timeout_timestamp.0,
        num_payloads = packet.payloads.len(),
        "packet fields for commitment recomputation"
    );
    for (i, p) in packet.payloads.iter().enumerate() {
        info!(
            payload_idx = i,
            source_port = %p.source_port.0,
            dest_port = %p.dest_port.0,
            version = %p.version,
            encoding = %p.encoding,
            data_len = p.data.len(),
            data_hex = %hex::encode(&p.data),
            "payload details"
        );
    }

    // Recompute commitment same as eureka ics24::packet_commitment_bytes32
    let expected_commitment = {
        let mut app_bytes = Vec::new();
        for p in &packet.payloads {
            let mut payload_buf = Vec::new();
            payload_buf.extend_from_slice(&sha256(p.source_port.0.as_bytes()));
            payload_buf.extend_from_slice(&sha256(p.dest_port.0.as_bytes()));
            payload_buf.extend_from_slice(&sha256(p.version.as_bytes()));
            payload_buf.extend_from_slice(&sha256(p.encoding.as_bytes()));
            payload_buf.extend_from_slice(&sha256(&p.data));
            app_bytes.extend_from_slice(&sha256(&payload_buf));
        }
        let dest_client_hash = sha256(packet.dest_client_id.0.as_bytes());
        let timeout_hash = sha256(&packet.timeout_timestamp.0.to_be_bytes());
        let app_hash = sha256(&app_bytes);
        let mut commitment_input = vec![0x02u8];
        commitment_input.extend_from_slice(&dest_client_hash);
        commitment_input.extend_from_slice(&timeout_hash);
        commitment_input.extend_from_slice(&app_hash);
        sha256(&commitment_input)
    };
    info!(
        expected_commitment_hex = %hex::encode(&expected_commitment),
        cosmos_commitment_hex = %commitment.as_ref().map(|c| hex::encode(&c.0)).unwrap_or_default(),
        commitments_match = (commitment.as_ref().map(|c| c.0.as_slice()) == Some(&expected_commitment[..])),
        "commitment comparison (eureka recomputation vs cosmos stored)"
    );

    let revision = harness.cosmos_chain.revision_number();

    let recv_msg =
        <SolanaAdapter as PacketMessageBuilder<CosmosChain<Secp256k1KeyPair>>>::build_receive_packet_message(
            &harness.solana_adapter,
            &packet,
            proof,
            target_height,
            revision,
        )
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;
    info!(
        %target_height,
        revision,
        "submitting recv_packet to Solana"
    );

    harness
        .solana_adapter
        .send_messages(vec![recv_msg])
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;
    info!("recv_packet submitted to Solana");

    assert_packet_receipt_written(&rpc, &harness, packet.sequence.0)?;
    assert_acknowledgement_written(&rpc, &harness, packet.sequence.0)?;

    info!("cosmos_to_solana_transfer: all assertions passed");
    Ok(())
}

async fn wait_for_height_past(
    chain: &CosmosChain<Secp256k1KeyPair>,
    min_height: u64,
) -> Result<tendermint::block::Height> {
    use std::time::Duration;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let latest = chain
            .query_latest_height()
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        if latest.value() > min_height {
            info!(latest = %latest, min_height, "cosmos chain advanced past min_height");
            return Ok(latest);
        }
        if tokio::time::Instant::now() >= deadline {
            eyre::bail!(
                "timed out waiting for cosmos to advance past height {min_height} (current: {})",
                latest.value()
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn send_ics20_packet(
    harness: &CosmosSolanaHarness,
) -> Result<(
    Vec<mercury_cosmos_counterparties::types::CosmosTxResponse>,
    u64,
)> {
    let wallet = harness.cosmos_handle.relayer_wallet();

    let packet_data = serde_json::json!({
        "denom": "stake",
        "amount": TRANSFER_AMOUNT.to_string(),
        "sender": wallet.address,
        "receiver": "solana_receiver_placeholder",
        "memo": "",
    });

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

    let height = responses
        .first()
        .ok_or_else(|| eyre::eyre!("no tx response"))?
        .height
        .value();

    Ok((responses, height))
}

fn extract_send_packet(
    responses: &[mercury_cosmos_counterparties::types::CosmosTxResponse],
) -> Result<CosmosPacket> {
    for response in responses {
        for event in &response.events {
            if let Some(SendPacketEvent { packet }) =
                <CosmosChain<Secp256k1KeyPair> as PacketEvents>::try_extract_send_packet_event(
                    event,
                )
            {
                return Ok(packet);
            }
        }
    }
    eyre::bail!("no SendPacket event found in tx responses")
}

fn get_client_latest_height(rpc: &RpcClient, harness: &CosmosSolanaHarness) -> Result<u64> {
    let (pda, _) = Ics07Tendermint::client_state_pda(&harness.solana_bootstrap.program_ids.ics07);
    let account = rpc
        .get_account(&pda)
        .map_err(|e| eyre::eyre!("ClientState PDA not found: {e}"))?;
    let cs: OnChainClientState = deserialize_anchor_account(&account.data)?;
    Ok(cs.latest_height.revision_height)
}

fn assert_client_state_exists(rpc: &RpcClient, harness: &CosmosSolanaHarness) -> Result<()> {
    let (pda, _) = Ics07Tendermint::client_state_pda(&harness.solana_bootstrap.program_ids.ics07);
    let account = rpc
        .get_account(&pda)
        .map_err(|e| eyre::eyre!("ClientState PDA not found: {e}"))?;
    assert!(!account.data.is_empty(), "ClientState PDA has empty data");
    info!(?pda, "ClientState PDA exists");
    Ok(())
}

fn assert_consensus_state_stored(
    rpc: &RpcClient,
    harness: &CosmosSolanaHarness,
    height: u64,
) -> Result<()> {
    let (pda, _) =
        Ics07Tendermint::consensus_state_pda(height, &harness.solana_bootstrap.program_ids.ics07);
    let account = rpc
        .get_account(&pda)
        .map_err(|e| eyre::eyre!("ConsensusState PDA at height {height} not found: {e}"))?;
    assert!(
        !account.data.is_empty(),
        "ConsensusState PDA has empty data"
    );
    info!(?pda, height, "ConsensusState PDA exists");
    Ok(())
}

fn assert_packet_receipt_written(
    rpc: &RpcClient,
    harness: &CosmosSolanaHarness,
    sequence: u64,
) -> Result<()> {
    let (pda, _) = Ics26Router::packet_receipt_pda(
        &harness.solana_tendermint_client_id,
        sequence,
        &harness.solana_bootstrap.program_ids.ics26,
    );
    let account = rpc
        .get_account(&pda)
        .map_err(|e| eyre::eyre!("PacketReceipt PDA for seq {sequence} not found: {e}"))?;
    assert!(!account.data.is_empty(), "PacketReceipt PDA has empty data");
    info!(?pda, sequence, "PacketReceipt PDA exists");
    Ok(())
}

fn assert_acknowledgement_written(
    rpc: &RpcClient,
    harness: &CosmosSolanaHarness,
    sequence: u64,
) -> Result<()> {
    let (pda, _) = Ics26Router::packet_ack_pda(
        &harness.solana_tendermint_client_id,
        sequence,
        &harness.solana_bootstrap.program_ids.ics26,
    );
    let account = rpc
        .get_account(&pda)
        .map_err(|e| eyre::eyre!("PacketAck PDA for seq {sequence} not found: {e}"))?;
    assert!(!account.data.is_empty(), "PacketAck PDA has empty data");
    info!(?pda, sequence, "PacketAck PDA exists");
    Ok(())
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}
