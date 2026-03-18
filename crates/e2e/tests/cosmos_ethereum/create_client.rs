use std::process::Command;

use eyre::Result;
use mercury_e2e::bootstrap::anvil::{self, AnvilHandle, start_anvil};
use mercury_e2e::bootstrap::cosmos_docker::{
    CosmosDockerBootstrap, CosmosDockerHandle, store_dummy_wasm_light_client,
};
use mercury_e2e::bootstrap::traits::{ChainBootstrap, ChainHandle};
use mercury_e2e::cosmos_eth_context::build_sp1_client_state;
use mercury_e2e::relayer::{find_or_build_binary, parse_client_id_from_stdout};

use super::*;

struct CrossChainInfra {
    cosmos_handle: CosmosDockerHandle,
    anvil_handle: AnvilHandle,
    wasm_checksum: String,
    sp1_light_client: alloy::primitives::Address,
    elf_dir: std::path::PathBuf,
}

#[allow(clippy::future_not_send)]
async fn setup_infra() -> Result<CrossChainInfra> {
    let cosmos_bootstrap = CosmosDockerBootstrap::new("mercury-cosmos");
    let cosmos_handle = cosmos_bootstrap.start().await?;
    let anvil_handle = start_anvil().await?;

    let wasm_checksum = store_dummy_wasm_light_client(&cosmos_handle).await?;

    let elf_dir = anvil::build_sp1_programs()?;
    let vkeys = anvil::derive_sp1_vkeys(&elf_dir)?;

    let (client_state_abi, consensus_state_hash) = build_sp1_client_state(&cosmos_handle).await?;

    let sp1_light_client = anvil::deploy_sp1_light_client(
        &anvil_handle.rpc_endpoint,
        &anvil_handle.relayer_wallet,
        anvil_handle.mock_verifier,
        &vkeys,
        &client_state_abi,
        consensus_state_hash,
    )?;

    Ok(CrossChainInfra {
        cosmos_handle,
        anvil_handle,
        wasm_checksum,
        sp1_light_client,
        elf_dir,
    })
}

fn write_config(
    config_dir: &tempfile::TempDir,
    infra: &CrossChainInfra,
) -> (std::path::PathBuf, String, String) {
    let cosmos_key_path = config_dir.path().join("key_cosmos.toml");
    std::fs::write(
        &cosmos_key_path,
        format!(
            "secret_key = \"{}\"",
            infra.cosmos_handle.relayer_wallet().secret_key_hex
        ),
    )
    .expect("write cosmos key");

    let eth_key_path = config_dir.path().join("key_eth.hex");
    std::fs::write(
        &eth_key_path,
        &infra.anvil_handle.relayer_wallet.private_key,
    )
    .expect("write eth key");

    let cosmos_chain_id = infra.cosmos_handle.chain_id().to_string();
    let eth_chain_id = infra.anvil_handle.chain_id.to_string();

    let config_path = config_dir.path().join("relayer.toml");
    let config = format!(
        r#"
[[chains]]
type = "cosmos"
chain_id = "{cosmos_chain_id}"
rpc_addr = "{rpc}"
grpc_addr = "{grpc}"
account_prefix = "cosmos"
key_name = "relayer"
key_file = "{cosmos_key}"
wasm_checksum = "{wasm_checksum}"
mock_proofs = true
[chains.gas_price]
amount = 0.0
denom = "stake"

[[chains]]
type = "ethereum"
chain_id = {eth_chain_id}
rpc_addr = "{eth_rpc}"
ics26_router = "{ics26:#x}"
key_file = "{eth_key}"
block_time_secs = 1
deployment_block = 0
light_client_address = "{sp1_lc:#x}"
[chains.client_payload_mode]
type = "mock"
[chains.sp1_prover]
elf_dir = "{elf_dir}"
zk_algorithm = "groth16"
prover_mode = "mock"
"#,
        rpc = infra.cosmos_handle.rpc_endpoint(),
        grpc = infra.cosmos_handle.grpc_endpoint(),
        cosmos_key = cosmos_key_path.display(),
        wasm_checksum = infra.wasm_checksum,
        eth_rpc = infra.anvil_handle.rpc_endpoint,
        ics26 = infra.anvil_handle.ics26_router,
        eth_key = eth_key_path.display(),
        sp1_lc = infra.sp1_light_client,
        elf_dir = infra.elf_dir.display(),
    );
    std::fs::write(&config_path, config).expect("write config");

    (config_path, cosmos_chain_id, eth_chain_id)
}

#[tokio::test]
#[ignore = "requires Docker and Foundry"]
async fn create_client_cosmos_host_eth_reference() {
    init_tracing();

    let infra = setup_infra().await.expect("infra setup");
    let config_dir = tempfile::tempdir().expect("create temp dir");
    let (config_path, cosmos_chain_id, eth_chain_id) = write_config(&config_dir, &infra);

    let binary = find_or_build_binary();

    let output = Command::new(&binary)
        .args([
            "create",
            "client",
            "--config",
            &config_path.to_string_lossy(),
            "--host-chain",
            &cosmos_chain_id,
            "--reference-chain",
            &eth_chain_id,
        ])
        .output()
        .expect("run create client");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "create client (cosmos host, eth ref) failed:\nstdout: {stdout}\nstderr: {stderr}"
    );

    let client_id = parse_client_id_from_stdout(&stdout);
    assert!(
        client_id
            .parse::<ibc::core::host::types::identifiers::ClientId>()
            .is_ok(),
        "invalid cosmos client ID: {client_id}"
    );
}

#[tokio::test]
#[ignore = "requires Docker and Foundry"]
async fn create_client_eth_host_cosmos_reference() {
    init_tracing();

    let infra = setup_infra().await.expect("infra setup");
    let config_dir = tempfile::tempdir().expect("create temp dir");
    let (config_path, cosmos_chain_id, eth_chain_id) = write_config(&config_dir, &infra);

    let binary = find_or_build_binary();

    let output = Command::new(&binary)
        .args([
            "create",
            "client",
            "--config",
            &config_path.to_string_lossy(),
            "--host-chain",
            &eth_chain_id,
            "--reference-chain",
            &cosmos_chain_id,
        ])
        .output()
        .expect("run create client");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "create client (eth host, cosmos ref) failed:\nstdout: {stdout}\nstderr: {stderr}"
    );

    let client_id = parse_client_id_from_stdout(&stdout);
    assert!(!client_id.is_empty(), "EVM client ID should not be empty");
}
