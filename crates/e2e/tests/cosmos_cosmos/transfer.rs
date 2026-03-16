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

    relay.stop();
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

    relay.stop();
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn packet_timeout() {
    super::init_tracing();

    let ctx = setup_context().await.expect("IBC setup");

    let user_a_addr = &ctx.handle_a.user_wallets()[0].address;
    let balance_before = TestContext::query_balance(&ctx.handle_a, user_a_addr, "stake")
        .await
        .expect("query balance on A before timeout transfer");

    // Start relay first so the event watcher sees the send_packet event.
    let relay = ctx.start_relay_library().expect("start relay");

    // Send with 1-second timeout. The event watcher stays 1 block behind tip,
    // so by the time the packet_worker sees the event (~2 blocks / ~2s later),
    // the timeout has already expired on the destination chain. The packet is
    // classified as timed-out on first observation and never delivered.
    ctx.send_transfer_a_to_b_with_timeout(1000, "stake", 1)
        .await
        .expect("transfer A→B with short timeout");

    // Relay should detect the timeout and process MsgTimeout, refunding sender
    ctx.assert_eventual_balance(
        &ctx.handle_a,
        user_a_addr,
        "stake",
        balance_before,
        Duration::from_secs(60),
    )
    .await
    .expect("balance refunded on A after timeout");

    relay.stop();
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn client_refresh_keeps_relay_alive() {
    super::init_tracing();

    let ctx = setup_context().await.expect("IBC setup");
    let relay = ctx.start_relay_library().expect("start relay");

    let user_b_addr = &ctx.handle_b.user_wallets()[0].address;
    let ibc_denom = TestContext::ibc_denom("transfer", &ctx.client_id_b.to_string(), "stake");

    ctx.send_transfer_a_to_b(1000, "stake")
        .await
        .expect("transfer A→B (first)");

    ctx.assert_eventual_balance(
        &ctx.handle_b,
        user_b_addr,
        &ibc_denom,
        1000,
        Duration::from_secs(60),
    )
    .await
    .expect("balance on B after first transfer");

    tokio::time::sleep(Duration::from_secs(10)).await;

    ctx.send_transfer_a_to_b(500, "stake")
        .await
        .expect("transfer A→B (second)");

    ctx.assert_eventual_balance(
        &ctx.handle_b,
        user_b_addr,
        &ibc_denom,
        1500,
        Duration::from_secs(60),
    )
    .await
    .expect("balance on B after second transfer");

    relay.stop();
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn concurrent_transfers() {
    super::init_tracing();

    let ctx = setup_context().await.expect("IBC setup");
    let relay = ctx.start_relay_library().expect("start relay");

    let user_b_addr = &ctx.handle_b.user_wallets()[0].address;
    let ibc_denom = TestContext::ibc_denom("transfer", &ctx.client_id_b.to_string(), "stake");

    for _ in 0..5 {
        ctx.send_transfer_a_to_b(100, "stake")
            .await
            .expect("transfer A→B");
    }

    ctx.assert_eventual_balance(
        &ctx.handle_b,
        user_b_addr,
        &ibc_denom,
        500,
        Duration::from_secs(120),
    )
    .await
    .expect("balance on B after concurrent transfers");

    relay.stop();
}
