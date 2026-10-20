use mercury_chain_traits::builders::{ClientMessageBuilder, ClientPayloadBuilder};
use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery};
use mercury_chain_traits::types::MessageSender;
use mercury_cosmos_counterparties::CosmosAdapter;
use mercury_cosmos_counterparties::chain::CosmosChain;
use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
use mercury_e2e::beacon_lc_context::BeaconLcTestContext;
use mercury_e2e::cosmos_eth_context::CosmosEthTestContext;
use mercury_ethereum::chain::EthereumChain;
use mercury_ethereum::types::EvmHeight;
use mercury_ethereum_counterparties::EthereumAdapter;

use super::*;

#[tokio::test]
#[ignore = "requires Docker and Foundry"]
async fn eth_client_on_cosmos_advances_height() -> Result<()> {
    init_tracing();
    let ctx = CosmosEthTestContext::setup().await?;

    let query_height = ctx.cosmos_chain.query_latest_height().await?;

    let initial_cs = ClientQuery::<EthereumChain>::query_client_state(
        &ctx.cosmos_chain,
        &ctx.client_id_on_cosmos,
        &query_height,
    )
    .await?;

    let initial_height =
        <CosmosAdapter<Secp256k1KeyPair> as ClientQuery<EthereumChain>>::client_latest_height(
            &initial_cs,
        );
    tracing::info!("Initial ETH client height on Cosmos: {initial_height:?}");

    let target_height = EvmHeight(initial_height.0 + 1);

    let update_payload =
        <EthereumAdapter as ClientPayloadBuilder<CosmosChain<Secp256k1KeyPair>>>::build_update_client_payload(
            &ctx.eth_chain, &initial_height, &target_height, &initial_cs,
        ).await?;

    assert!(
        !update_payload.headers.is_empty(),
        "mock payload must produce at least one header"
    );

    let update_output = ClientMessageBuilder::<EthereumChain>::build_update_client_message(
        &ctx.cosmos_chain,
        &ctx.client_id_on_cosmos,
        update_payload,
    )
    .await?;

    ctx.cosmos_chain
        .send_messages(update_output.messages)
        .await?;

    let updated_query_height = ctx.cosmos_chain.query_latest_height().await?;
    let updated_cs = ClientQuery::<EthereumChain>::query_client_state(
        &ctx.cosmos_chain,
        &ctx.client_id_on_cosmos,
        &updated_query_height,
    )
    .await?;
    let updated_height =
        <CosmosAdapter<Secp256k1KeyPair> as ClientQuery<EthereumChain>>::client_latest_height(
            &updated_cs,
        );
    tracing::info!("Updated ETH client height on Cosmos: {updated_height:?}");

    assert!(
        updated_height.0 > initial_height.0,
        "client height must advance: was {initial_height:?}, now {updated_height:?}"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "requires Kurtosis"]
async fn eth_client_on_cosmos_advances_height_beacon() -> Result<()> {
    init_tracing();
    let ctx = BeaconLcTestContext::setup().await?;

    let query_height = ctx.cosmos_chain.query_latest_height().await?;

    let initial_cs = ClientQuery::<EthereumChain>::query_client_state(
        &ctx.cosmos_chain,
        &ctx.client_id_on_cosmos,
        &query_height,
    )
    .await?;

    let initial_height =
        <CosmosAdapter<Secp256k1KeyPair> as ClientQuery<EthereumChain>>::client_latest_height(
            &initial_cs,
        );
    tracing::info!("Initial beacon ETH client height on Cosmos: {initial_height:?}");

    tokio::time::sleep(std::time::Duration::from_secs(30)).await;

    let target_height = EvmHeight(initial_height.0 + 32);

    let update_payload =
        <EthereumAdapter as ClientPayloadBuilder<CosmosChain<Secp256k1KeyPair>>>::build_update_client_payload(
            &ctx.eth_chain, &initial_height, &target_height, &initial_cs,
        ).await?;

    assert!(
        !update_payload.headers.is_empty(),
        "beacon API must return at least one header"
    );

    // The light client rejects headers signed after the Cosmos block time,
    // so wait like the relay workers do.
    if let Some(required_ts) = <EthereumAdapter as ClientPayloadBuilder<
        CosmosChain<Secp256k1KeyPair>,
    >>::required_dst_timestamp_secs(&ctx.eth_chain, &update_payload)
    {
        wait_for_cosmos_timestamp(&ctx.cosmos_chain, required_ts).await?;
    }

    let update_output = ClientMessageBuilder::<EthereumChain>::build_update_client_message(
        &ctx.cosmos_chain,
        &ctx.client_id_on_cosmos,
        update_payload,
    )
    .await?;

    ctx.cosmos_chain
        .send_messages(update_output.messages)
        .await?;

    let updated_query_height = ctx.cosmos_chain.query_latest_height().await?;
    let updated_cs = ClientQuery::<EthereumChain>::query_client_state(
        &ctx.cosmos_chain,
        &ctx.client_id_on_cosmos,
        &updated_query_height,
    )
    .await?;
    let updated_height =
        <CosmosAdapter<Secp256k1KeyPair> as ClientQuery<EthereumChain>>::client_latest_height(
            &updated_cs,
        );
    tracing::info!("Updated beacon ETH client height on Cosmos: {updated_height:?}");

    assert!(
        updated_height.0 > initial_height.0,
        "client height must advance: was {initial_height:?}, now {updated_height:?}"
    );

    Ok(())
}

async fn wait_for_cosmos_timestamp<C: ChainStatusQuery>(chain: &C, required_ts: u64) -> Result<()> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let status = chain.query_chain_status().await?;
        let ts = C::chain_status_timestamp_secs(&status);
        if ts >= required_ts {
            return Ok(());
        }
        eyre::ensure!(
            tokio::time::Instant::now() < deadline,
            "cosmos timestamp {ts} did not reach {required_ts}"
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
