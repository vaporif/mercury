use alloy::providers::Provider;

use super::*;
use mercury_e2e::bootstrap::anvil::start_anvil;

#[tokio::test]
#[ignore = "requires Foundry"]
async fn anvil_bootstrap_smoke() -> Result<()> {
    init_tracing();

    let handle = start_anvil().await?;

    assert!(handle.chain_id() > 0);
    assert!(handle.rpc_endpoint().starts_with("http://"));
    assert_ne!(handle.ics26_router, alloy::primitives::Address::ZERO);
    assert_ne!(handle.ics20_transfer, alloy::primitives::Address::ZERO);
    assert_ne!(handle.mock_verifier, alloy::primitives::Address::ZERO);
    assert_ne!(handle.erc20, alloy::primitives::Address::ZERO);

    let provider = alloy::providers::ProviderBuilder::new()
        .connect_http(handle.rpc_endpoint().parse().expect("valid RPC URL"))
        .erased();
    let chain_id = provider.get_chain_id().await?;
    assert_eq!(chain_id, handle.chain_id());

    Ok(())
}
