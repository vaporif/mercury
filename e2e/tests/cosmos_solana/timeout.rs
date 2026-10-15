use std::sync::Arc;
use std::time::Duration;

use eyre::Result;
use ibc::core::host::types::identifiers::ClientId;
use mercury_chain_cache::CachedChain;
use mercury_chain_traits::queries::{ChainStatusQuery, PacketStateQuery};
use mercury_chain_traits::types::PacketSequence;
use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
use mercury_cosmos_counterparties::wrapper::CosmosAdapter;
use mercury_relay::context::{RelayContext, RelayWorkerConfig};
use mercury_solana::accounts::Ics26Router;
use mercury_solana::types::SolanaClientId;
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::bidirectional::{
    poll_until, poll_until_async, send_ics20_packet_from_cosmos_with_timeout, unix_now,
};
use super::bootstrap::set_up_cosmos_solana;
use super::init_tracing;

const PACKET_TIMEOUT_SECS: u64 = 10;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Docker and solana-test-validator"]
async fn timeout_cosmos_to_solana() -> Result<()> {
    // A packet that expires before the relayer starts is timed out on Cosmos,
    // proven by an attestation that Solana never wrote a receipt.
    init_tracing();

    let fixtures_dir = super::solana_fixtures_dir()?;
    let harness = set_up_cosmos_solana(&fixtures_dir).await?;

    let cosmos_client_id: ClientId = harness
        .cosmos_client_id
        .parse()
        .map_err(|e| eyre::eyre!("parse cosmos client id: {e}"))?;
    let solana_client_id = SolanaClientId(harness.solana_tendermint_client_id.clone());

    let timeout = unix_now() + PACKET_TIMEOUT_SECS;
    send_ics20_packet_from_cosmos_with_timeout(&harness, timeout).await?;
    info!(timeout, "sent Cosmos->Solana packet that will time out");

    let rpc = RpcClient::new_with_commitment(
        harness.solana_bootstrap.rpc_url.clone(),
        CommitmentConfig::confirmed(),
    );
    poll_until(
        "Solana block time past the packet timeout",
        Duration::from_secs(60),
        || {
            rpc.get_slot()
                .and_then(|slot| rpc.get_block_time(slot))
                .is_ok_and(|t| u64::try_from(t).is_ok_and(|t| t > timeout))
        },
    )
    .await?;

    let relay = Arc::new(RelayContext {
        src_chain: CachedChain::new(CosmosAdapter::<Secp256k1KeyPair>(
            harness.cosmos_chain.clone(),
        )),
        dst_chain: CachedChain::new(harness.solana_adapter.clone()),
        src_client_id: cosmos_client_id.clone(),
        dst_client_id: solana_client_id,
    });
    let token = CancellationToken::new();
    // Start the event watcher far enough back to see the send event; it was
    // emitted before the relayer started.
    let config = RelayWorkerConfig {
        lookback: Some(Duration::from_secs(120)),
        ..RelayWorkerConfig::default()
    };
    let join = tokio::spawn({
        let relay = Arc::clone(&relay);
        let t = token.clone();
        async move { relay.run_with_token(t, config).await }
    });

    poll_until_async(
        "packet commitment cleared on Cosmos (timeout)",
        Duration::from_secs(120),
        || {
            let chain = harness.cosmos_chain.clone();
            let client_id = cosmos_client_id.clone();
            async move {
                let Ok(height) = chain.query_latest_height().await else {
                    return false;
                };
                chain
                    .query_packet_commitment(&client_id, PacketSequence(1), &height)
                    .await
                    .is_ok_and(|(commitment, _)| commitment.is_none())
            }
        },
    )
    .await?;
    info!("Cosmos->Solana: timeout relayed, packet commitment cleared on Cosmos");

    let (receipt_pda, _) = Ics26Router::packet_receipt_pda(
        &harness.solana_tendermint_client_id,
        1,
        &harness.solana_bootstrap.program_ids.ics26,
    );
    assert!(
        rpc.get_account(&receipt_pda).is_err(),
        "timed-out packet must not be received on Solana"
    );

    token.cancel();
    join.abort();
    info!("timeout_cosmos_to_solana: all assertions passed");
    Ok(())
}
