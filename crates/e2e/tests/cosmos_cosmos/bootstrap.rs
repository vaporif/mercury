use mercury_e2e::bootstrap::cosmos_docker::CosmosDockerBootstrap;
use mercury_e2e::bootstrap::traits::{ChainBootstrap, ChainHandle};
use tendermint_rpc::Client;

#[tokio::test]
#[ignore = "requires Docker"]
async fn bootstrap_smoke() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let bootstrap = CosmosDockerBootstrap::new("mercury-test");
    let handle = bootstrap.start().await.expect("bootstrap should succeed");

    assert!(!handle.chain_id().is_empty());
    assert!(handle.rpc_endpoint().starts_with("http://"));
    assert!(handle.grpc_endpoint().starts_with("http://"));

    let relayer = handle.relayer_wallet();
    assert!(
        !relayer.secret_key_hex.is_empty(),
        "relayer key should not be empty"
    );
    assert!(
        relayer.address.starts_with("cosmos"),
        "relayer address should have cosmos prefix"
    );
    assert_eq!(
        relayer.secret_key_hex.len(),
        64,
        "hex key should be 64 chars (32 bytes)"
    );

    let users = handle.user_wallets();
    assert_eq!(users.len(), 2);
    for user in users {
        assert!(!user.secret_key_hex.is_empty());
        assert!(user.address.starts_with("cosmos"));
    }

    let rpc_client = tendermint_rpc::HttpClient::new(handle.rpc_endpoint()).unwrap();
    let status = rpc_client.status().await.expect("RPC should be reachable");
    assert_eq!(status.node_info.network.as_str(), handle.chain_id());
}
