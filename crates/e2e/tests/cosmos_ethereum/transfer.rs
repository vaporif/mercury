use std::time::Duration;

use mercury_e2e::cosmos_eth_context::CosmosEthTestContext;

use super::*;

#[tokio::test]
#[ignore = "requires Docker and Foundry"]
async fn cosmos_to_eth_transfer() -> Result<()> {
    init_tracing();

    let ctx = CosmosEthTestContext::setup().await?;
    let relay = ctx.start_relay_library()?;

    // Send 1000 stake from Cosmos user1 → Ethereum user1
    ctx.send_cosmos_to_eth_transfer(1000, "stake").await?;

    // The IBC denom on Ethereum: transfer/{eth_client_id}/stake
    let eth_denom = format!("transfer/{}/stake", ctx.client_id_on_eth);
    let eth_user = ctx.anvil_handle.user_wallets[0].address;

    ctx.assert_eventual_eth_balance(&eth_denom, eth_user, 1000, Duration::from_secs(120))
        .await?;

    relay.stop();

    Ok(())
}
