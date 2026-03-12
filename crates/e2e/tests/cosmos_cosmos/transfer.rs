use std::time::Duration;

use mercury_e2e::bootstrap::traits::ChainHandle;
use mercury_e2e::context::TestContext;

use super::setup_context;

#[tokio::test]
#[ignore = "requires Docker"]
async fn ibc_transfer() {
    tracing_subscriber::fmt()
        .with_env_filter("info,mercury_relay=debug,mercury_cosmos::events=debug,mercury_e2e=debug")
        .init();

    let ctx = setup_context().await.expect("IBC setup");
    let relay = ctx.start_relay_library().expect("start relay");

    ctx.send_transfer_a_to_b(1000, "stake")
        .await
        .expect("transfer A→B");

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

    relay.stop().await.expect("stop relay");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn bidirectional_transfer() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let ctx = setup_context().await.expect("IBC setup");
    let relay = ctx.start_relay_library().expect("start relay");

    // Transfer A → B (1000 stake)
    ctx.send_transfer_a_to_b(1000, "stake")
        .await
        .expect("transfer A→B");

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

    // TODO: Transfer B → A (500 of IBC tokens) and assert A received tokens back

    relay.stop().await.expect("stop relay");
}
