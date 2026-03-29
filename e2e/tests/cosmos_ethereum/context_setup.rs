use super::*;
use mercury_e2e::bootstrap::traits::ChainHandle;
use mercury_e2e::cosmos_eth_context::CosmosEthTestContext;

#[tokio::test]
#[ignore = "requires Docker and Foundry"]
async fn context_setup_smoke() -> Result<()> {
    init_tracing();

    let ctx = CosmosEthTestContext::setup().await?;

    assert!(!ctx.cosmos_handle.chain_id().is_empty());
    assert!(ctx.anvil_handle.rpc_endpoint().starts_with("http://"));

    Ok(())
}
