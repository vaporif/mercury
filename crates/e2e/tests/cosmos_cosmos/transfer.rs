use std::time::Duration;

use mercury_e2e::bootstrap::traits::ChainHandle;
use mercury_e2e::context::TestContext;

use super::setup_context;

#[tokio::test]
#[ignore = "requires Docker"]
async fn ibc_transfer() {
    super::init_tracing();

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
    super::init_tracing();

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

    // Transfer B → A (500 of the IBC tokens just received)
    let user_a_addr = &ctx.handle_a.user_wallets()[0].address;

    // Snapshot A's native stake balance before the return transfer
    let balance_before = TestContext::query_balance(&ctx.handle_a, user_a_addr, "stake")
        .await
        .expect("query balance on A before B→A");

    // IBC v2 packet data uses the trace path, not the ibc/HASH bank denom
    let trace_path =
        TestContext::denom_trace_path("transfer", &ctx.client_id_b.to_string(), "stake");
    ctx.send_transfer_b_to_a(500, &trace_path)
        .await
        .expect("transfer B→A");

    // On chain A the tokens are un-escrowed back to native "stake"
    ctx.assert_eventual_balance(
        &ctx.handle_a,
        user_a_addr,
        "stake",
        balance_before + 500,
        Duration::from_secs(60),
    )
    .await
    .expect("balance on A after B→A transfer");

    relay.stop().await.expect("stop relay");
}
