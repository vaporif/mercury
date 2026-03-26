use std::time::Duration;

use alloy::primitives::Address;
use eyre::{Context, Result, bail, ensure};
use serde_json::json;
use tokio::sync::OnceCell;
use tracing::info;
use uuid::Uuid;

const FINALIZATION_TIMEOUT: Duration = Duration::from_secs(300);
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const KURTOSIS_CHAIN_ID: u64 = 3_151_908;

// ethpandaops/ethereum-package faucet key — pre-funded in genesis
const PRE_FUNDED_PRIVATE_KEY: &str =
    "04b9f63ecf84210c5366c66d68fa1f5da1fa4f634fad6dfc86178e4d79ff9e59";
const PRE_FUNDED_ADDRESS: &str = "0xafF0CA253b97e54440965855cec0A8a2E2399896";

pub struct KurtosisHandle {
    pub beacon_api_url: String,
    pub el_rpc_url: String,
    pub enclave_name: String,
    pub chain_id: u64,
    pub pre_funded_key: PreFundedKey,
}

pub struct PreFundedKey {
    pub private_key: String,
    pub address: Address,
}

static KURTOSIS: OnceCell<KurtosisHandle> = OnceCell::const_new();

pub async fn get_or_init_kurtosis() -> Result<&'static KurtosisHandle> {
    KURTOSIS.get_or_try_init(start_kurtosis).await
}

fn kurtosis_config() -> String {
    json!({
        "participants": [{
            "cl_type": "lodestar",
            "el_type": "geth",
            "count": 1,
            "el_extra_params": ["--http.api=admin,eth,net,web3,debug"]
        }],
        "network_params": {
            "preset": "minimal",
            "seconds_per_slot": 2,
            "num_validator_keys_per_node": 512,
            "genesis_delay": 20
        },
        "additional_services": []
    })
    .to_string()
}

async fn start_kurtosis() -> Result<KurtosisHandle> {
    let short_uuid = &Uuid::new_v4().to_string()[..8];
    let enclave_name = format!("mercury-e2e-{short_uuid}");

    let config_file = tempfile::NamedTempFile::new().wrap_err("creating temp config file")?;
    std::fs::write(config_file.path(), kurtosis_config()).wrap_err("writing kurtosis config")?;

    info!(enclave = %enclave_name, "starting Kurtosis Ethereum devnet");

    let output = tokio::process::Command::new("kurtosis")
        .args([
            "run",
            "github.com/ethpandaops/ethereum-package",
            "--enclave",
            &enclave_name,
            "--args-file",
            &config_file.path().to_string_lossy(),
        ])
        .output()
        .await
        .wrap_err("running kurtosis — is kurtosis installed?")?;

    ensure!(
        output.status.success(),
        "kurtosis run failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let beacon_api_url = kurtosis_port_print(&enclave_name, "cl-1-lodestar-geth", "http").await?;
    let el_rpc_raw = kurtosis_port_print(&enclave_name, "el-1-geth-lodestar", "rpc").await?;
    let el_rpc_url = if el_rpc_raw.starts_with("http") {
        el_rpc_raw
    } else {
        format!("http://{el_rpc_raw}")
    };

    info!(beacon = %beacon_api_url, el = %el_rpc_url, "Kurtosis ports resolved");

    wait_for_finalization_and_sync_committee(&beacon_api_url).await?;

    Ok(KurtosisHandle {
        beacon_api_url,
        el_rpc_url,
        enclave_name,
        chain_id: KURTOSIS_CHAIN_ID,
        pre_funded_key: PreFundedKey {
            private_key: PRE_FUNDED_PRIVATE_KEY.to_string(),
            address: PRE_FUNDED_ADDRESS
                .parse()
                .expect("valid pre-funded address"),
        },
    })
}

async fn kurtosis_port_print(enclave: &str, service: &str, port_id: &str) -> Result<String> {
    let output = tokio::process::Command::new("kurtosis")
        .args(["port", "print", enclave, service, port_id])
        .output()
        .await
        .wrap_err_with(|| format!("kurtosis port print {service} {port_id}"))?;

    ensure!(
        output.status.success(),
        "kurtosis port print failed for {service}/{port_id}:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn wait_for_finalization_and_sync_committee(beacon_api_url: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let start = std::time::Instant::now();

    info!("waiting for beacon chain finalization");

    loop {
        if start.elapsed() > FINALIZATION_TIMEOUT {
            bail!("beacon chain did not finalize within {FINALIZATION_TIMEOUT:?}");
        }

        let finalized = check_finalization(&client, beacon_api_url).await;
        if finalized.unwrap_or(false) {
            break;
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }

    info!(
        elapsed = ?start.elapsed(),
        "finalization reached, checking sync committee data"
    );

    loop {
        if start.elapsed() > FINALIZATION_TIMEOUT {
            bail!("sync committee data not available within {FINALIZATION_TIMEOUT:?}");
        }

        let has_sync = check_sync_committee_update(&client, beacon_api_url).await;
        if has_sync.unwrap_or(false) {
            break;
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }

    info!(elapsed = ?start.elapsed(), "beacon chain ready");
    Ok(())
}

async fn check_finalization(client: &reqwest::Client, beacon_api_url: &str) -> Result<bool> {
    let url = format!("{beacon_api_url}/eth/v1/beacon/states/head/finality_checkpoints");
    let resp: serde_json::Value = client.get(&url).send().await?.json().await?;

    let epoch = resp
        .pointer("/data/finalized/epoch")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    Ok(epoch > 0)
}

async fn check_sync_committee_update(
    client: &reqwest::Client,
    beacon_api_url: &str,
) -> Result<bool> {
    let url = format!("{beacon_api_url}/eth/v1/beacon/light_client/updates?start_period=0&count=1");
    let resp: serde_json::Value = client.get(&url).send().await?.json().await?;

    let has_finalized_header = resp
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|item| item.pointer("/data/finalized_header"))
        .is_some();

    Ok(has_finalized_header)
}
