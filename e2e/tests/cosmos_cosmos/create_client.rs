use std::process::Command;

use mercury_e2e::bootstrap::cosmos_docker::CosmosDockerBootstrap;
use mercury_e2e::bootstrap::traits::{ChainBootstrap, ChainHandle};
use mercury_e2e::relayer::find_or_build_binary;

struct CosmosCosmosInfra {
    handle_a: mercury_e2e::bootstrap::cosmos_docker::CosmosDockerHandle,
    handle_b: mercury_e2e::bootstrap::cosmos_docker::CosmosDockerHandle,
    config_path: std::path::PathBuf,
    _config_dir: tempfile::TempDir,
}

#[allow(clippy::future_not_send)]
async fn setup_infra() -> CosmosCosmosInfra {
    let bootstrap_a = CosmosDockerBootstrap::new("mercury-a");
    let bootstrap_b = CosmosDockerBootstrap::new("mercury-b");
    let (handle_a, handle_b) =
        tokio::try_join!(bootstrap_a.start(), bootstrap_b.start()).expect("bootstrap both chains");

    let config_dir = tempfile::tempdir().expect("create temp dir");

    let key_path_a = config_dir.path().join("key_a.toml");
    let key_path_b = config_dir.path().join("key_b.toml");
    std::fs::write(
        &key_path_a,
        format!(
            "secret_key = \"{}\"",
            handle_a.relayer_wallet().secret_key_hex
        ),
    )
    .expect("write key_a");
    std::fs::write(
        &key_path_b,
        format!(
            "secret_key = \"{}\"",
            handle_b.relayer_wallet().secret_key_hex
        ),
    )
    .expect("write key_b");

    let config_path = config_dir.path().join("relayer.toml");
    let config = format!(
        r#"
[[chains]]
type = "cosmos"
chain_id = "{chain_id_a}"
rpc_addr = "{rpc_a}"
grpc_addr = "{grpc_a}"
account_prefix = "cosmos"
key_name = "relayer"
key_file = "{key_a}"
[chains.gas_price]
amount = 0.0
denom = "stake"

[[chains]]
type = "cosmos"
chain_id = "{chain_id_b}"
rpc_addr = "{rpc_b}"
grpc_addr = "{grpc_b}"
account_prefix = "cosmos"
key_name = "relayer"
key_file = "{key_b}"
[chains.gas_price]
amount = 0.0
denom = "stake"
"#,
        chain_id_a = handle_a.chain_id(),
        rpc_a = handle_a.rpc_endpoint(),
        grpc_a = handle_a.grpc_endpoint(),
        key_a = key_path_a.display(),
        chain_id_b = handle_b.chain_id(),
        rpc_b = handle_b.rpc_endpoint(),
        grpc_b = handle_b.grpc_endpoint(),
        key_b = key_path_b.display(),
    );
    std::fs::write(&config_path, config).expect("write config");

    CosmosCosmosInfra {
        handle_a,
        handle_b,
        config_path,
        _config_dir: config_dir,
    }
}

fn assert_create_client(binary: &str, config_path: &std::path::Path, host: &str, reference: &str) {
    let output = Command::new(binary)
        .args([
            "create",
            "client",
            "--config",
            &config_path.to_string_lossy(),
            "--host-chain",
            host,
            "--reference-chain",
            reference,
        ])
        .output()
        .expect("run create client");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "create client failed (host={host}, ref={reference}):\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn create_client_b_tracks_a() {
    super::init_tracing();

    let infra = setup_infra().await;
    let binary = find_or_build_binary();

    assert_create_client(
        &binary,
        &infra.config_path,
        infra.handle_b.chain_id(),
        infra.handle_a.chain_id(),
    );
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn create_client_a_tracks_b() {
    super::init_tracing();

    let infra = setup_infra().await;
    let binary = find_or_build_binary();

    assert_create_client(
        &binary,
        &infra.config_path,
        infra.handle_a.chain_id(),
        infra.handle_b.chain_id(),
    );
}
