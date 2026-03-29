use std::time::Duration;

use mercury_e2e::bootstrap::traits::ChainHandle;
use mercury_e2e::cosmos_eth_context::CosmosEthTestContext;

use super::*;

#[tokio::test]
#[ignore = "requires Docker and Foundry"]
async fn cosmos_to_eth_transfer() -> Result<()> {
    init_tracing();

    let ctx = CosmosEthTestContext::setup().await?;
    let relay = ctx.start_relay_library()?;

    ctx.send_cosmos_to_eth_transfer(1000, "stake").await?;

    let eth_denom = format!("transfer/{}/stake", ctx.client_id_on_eth);
    let eth_user = ctx.anvil_handle.user_wallets[0].address;

    ctx.assert_eventual_eth_balance(&eth_denom, eth_user, 1000, Duration::from_secs(60))
        .await?;

    relay.stop();

    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker and Foundry"]
async fn eth_to_cosmos_transfer() -> Result<()> {
    init_tracing();

    let ctx = CosmosEthTestContext::setup().await?;
    let relay = ctx.start_relay_library()?;

    ctx.send_cosmos_to_eth_transfer(1000, "stake").await?;

    let eth_denom = format!("transfer/{}/stake", ctx.client_id_on_eth);
    let eth_user = ctx.anvil_handle.user_wallets[0].address;

    ctx.assert_eventual_eth_balance(&eth_denom, eth_user, 1000, Duration::from_secs(60))
        .await?;

    let cosmos_user_addr = &ctx.cosmos_handle.user_wallets()[0].address;
    let balance_before = ctx.query_cosmos_balance(cosmos_user_addr, "stake").await?;

    ctx.send_eth_to_cosmos_transfer(1000, &eth_denom).await?;

    ctx.assert_eventual_cosmos_balance(
        cosmos_user_addr,
        "stake",
        balance_before + 1000,
        Duration::from_secs(60),
    )
    .await?;

    relay.stop();

    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker and Foundry"]
async fn cosmos_eth_roundtrip_transfer() -> Result<()> {
    init_tracing();

    let ctx = CosmosEthTestContext::setup().await?;
    let relay = ctx.start_relay_library()?;

    ctx.send_cosmos_to_eth_transfer(1000, "stake").await?;

    let eth_denom = format!("transfer/{}/stake", ctx.client_id_on_eth);
    let eth_user = ctx.anvil_handle.user_wallets[0].address;

    ctx.assert_eventual_eth_balance(&eth_denom, eth_user, 1000, Duration::from_secs(60))
        .await?;

    let cosmos_user_addr = &ctx.cosmos_handle.user_wallets()[0].address;
    let balance_before = ctx.query_cosmos_balance(cosmos_user_addr, "stake").await?;

    ctx.send_eth_to_cosmos_transfer(1000, &eth_denom).await?;

    ctx.assert_eventual_cosmos_balance(
        cosmos_user_addr,
        "stake",
        balance_before + 1000,
        Duration::from_secs(60),
    )
    .await?;

    relay.stop();

    Ok(())
}
