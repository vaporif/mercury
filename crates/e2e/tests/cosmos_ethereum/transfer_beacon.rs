use std::time::Duration;

use mercury_e2e::beacon_lc_context::BeaconLcTestContext;
use mercury_e2e::bootstrap::traits::ChainHandle;

use super::*;

#[tokio::test]
#[ignore = "requires Kurtosis"]
async fn cosmos_to_eth_transfer_beacon() -> Result<()> {
    init_tracing();

    let ctx = BeaconLcTestContext::setup().await?;
    let relay = ctx.start_relay_library()?;

    ctx.send_cosmos_to_eth_transfer(1000, "stake").await?;

    let eth_denom = format!("transfer/{}/stake", ctx.client_id_on_eth);
    let eth_user = ctx.eth_user_wallet.address;

    ctx.assert_eventual_eth_balance(&eth_denom, eth_user, 1000, Duration::from_secs(300))
        .await?;

    relay.stop();

    Ok(())
}

#[tokio::test]
#[ignore = "requires Kurtosis"]
async fn cosmos_eth_roundtrip_transfer_beacon() -> Result<()> {
    init_tracing();

    let ctx = BeaconLcTestContext::setup().await?;
    let relay = ctx.start_relay_library()?;

    ctx.send_cosmos_to_eth_transfer(1000, "stake").await?;

    let eth_denom = format!("transfer/{}/stake", ctx.client_id_on_eth);
    let eth_user = ctx.eth_user_wallet.address;

    ctx.assert_eventual_eth_balance(&eth_denom, eth_user, 1000, Duration::from_secs(300))
        .await?;

    let cosmos_user_addr = &ctx.cosmos_handle.user_wallets()[0].address;
    let balance_before = ctx.query_cosmos_balance(cosmos_user_addr, "stake").await?;

    ctx.send_eth_to_cosmos_transfer(1000, &eth_denom).await?;

    // Beacon LC relay requires waiting for the beacon finality_update endpoint
    // to advance past sync committee period boundaries, which can be slow in
    // Kurtosis minimal preset (~5 min after each period crossing).
    ctx.assert_eventual_cosmos_balance(
        cosmos_user_addr,
        "stake",
        balance_before + 1000,
        Duration::from_secs(600),
    )
    .await?;

    relay.stop();

    Ok(())
}
