use std::time::Duration;

use mercury_e2e::bootstrap::traits::ChainHandle;
use mercury_e2e::context::TestContext;

use super::setup_context;

#[tokio::test]
#[ignore = "requires Docker"]
async fn binary_smoke() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let ctx = setup_context().await.expect("IBC setup");
    let relay = ctx.start_relay_binary().expect("start binary relay");

    // Wait for relayer to initialize
    tokio::time::sleep(Duration::from_secs(5)).await;

    ctx.send_transfer_a_to_b(1000, "stake")
        .await
        .expect("transfer");

    let user_b_addr = &ctx.handle_b.user_wallets()[0].address;
    let ibc_denom = TestContext::ibc_denom("transfer", &ctx.client_id_b.to_string(), "stake");

    ctx.assert_eventual_balance(
        &ctx.handle_b,
        user_b_addr,
        &ibc_denom,
        1000,
        Duration::from_secs(60),
    )
    .await
    .expect("balance on B");

    relay.stop().expect("stop relay");
}
