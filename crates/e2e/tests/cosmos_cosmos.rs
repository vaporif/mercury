use std::sync::Once;

use eyre::Result;
use mercury_e2e::bootstrap::cosmos_docker::CosmosDockerBootstrap;
use mercury_e2e::bootstrap::traits::ChainBootstrap;
use mercury_e2e::context::TestContext;

#[path = "cosmos_cosmos/binary.rs"]
mod binary;
#[path = "cosmos_cosmos/bootstrap.rs"]
mod bootstrap;
#[path = "cosmos_cosmos/transfer.rs"]
mod transfer;

static INIT_TRACING: Once = Once::new();

fn init_tracing() {
    INIT_TRACING.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter(
                "info,mercury_relay=debug,mercury_cosmos::events=debug,mercury_e2e=debug",
            )
            .init();
    });
}

#[allow(clippy::future_not_send)]
async fn setup_context() -> Result<TestContext> {
    let bootstrap_a = CosmosDockerBootstrap::new("mercury-a");
    let bootstrap_b = CosmosDockerBootstrap::new("mercury-b");

    let handle_a = bootstrap_a.start().await?;
    let handle_b = bootstrap_b.start().await?;

    TestContext::setup(handle_a, handle_b).await
}
