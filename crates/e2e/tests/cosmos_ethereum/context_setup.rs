use super::*;
use mercury_e2e::bootstrap::traits::ChainHandle;
use mercury_e2e::cosmos_eth_context::CosmosEthTestContext;

#[tokio::test]
#[ignore = "requires Docker and Foundry"]
async fn context_setup_smoke() -> Result<()> {
    init_tracing();

    let ctx = CosmosEthTestContext::setup().await?;

    // Verify client IDs are real (not placeholders)
    assert!(
        !ctx.client_id_on_cosmos.to_string().is_empty(),
        "cosmos client ID should not be empty"
    );
    assert!(
        !ctx.client_id_on_eth.to_string().is_empty(),
        "ethereum client ID should not be empty"
    );

    // Verify chains are accessible
    assert!(!ctx.cosmos_handle.chain_id().is_empty());
    assert!(ctx.anvil_handle.rpc_endpoint().starts_with("http://"));

    Ok(())
}
