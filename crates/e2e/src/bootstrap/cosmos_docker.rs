use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use eyre::{Context, Result, bail};
use testcontainers::core::{ExecCommand, IntoContainerPort};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tracing::{info, warn};
use uuid::Uuid;

use super::traits::{ChainBootstrap, ChainHandle, Wallet};

const IMAGE: &str = "ghcr.io/cosmos/ibc-go-wasm-simd";
const IMAGE_TAG: &str = "main";
const RPC_PORT: u16 = 26657;
const GRPC_PORT: u16 = 9090;
const READINESS_TIMEOUT: Duration = Duration::from_secs(60);
const READINESS_POLL_INTERVAL: Duration = Duration::from_secs(1);
const READINESS_WARNING_THRESHOLD: Duration = Duration::from_secs(15);

pub struct CosmosDockerBootstrap {
    pub chain_id_prefix: String,
}

impl CosmosDockerBootstrap {
    #[must_use]
    pub fn new(prefix: &str) -> Self {
        Self {
            chain_id_prefix: prefix.to_string(),
        }
    }

    fn generate_chain_id(&self) -> String {
        let short_uuid = &Uuid::new_v4().to_string()[..8];
        format!("{}-{}", self.chain_id_prefix, short_uuid)
    }
}

fn generate_init_script(chain_id: &str) -> String {
    format!(
        r#"#!/bin/sh
set -e
CHAIN_ID="{chain_id}"
BINARY="simd"
HOME_DIR="/root/.simapp"

$BINARY init test --chain-id $CHAIN_ID --home $HOME_DIR 2>/dev/null

# Create wallets
$BINARY keys add validator --keyring-backend test --home $HOME_DIR 2>/dev/null
$BINARY keys add relayer --keyring-backend test --home $HOME_DIR 2>/dev/null
$BINARY keys add user1 --keyring-backend test --home $HOME_DIR 2>/dev/null
$BINARY keys add user2 --keyring-backend test --home $HOME_DIR 2>/dev/null

# Export raw hex private keys
mkdir -p /keys
yes | $BINARY keys export validator --unarmored-hex --unsafe --keyring-backend test --home $HOME_DIR 2>/dev/null > /keys/validator.hex
yes | $BINARY keys export relayer --unarmored-hex --unsafe --keyring-backend test --home $HOME_DIR 2>/dev/null > /keys/relayer.hex
yes | $BINARY keys export user1 --unarmored-hex --unsafe --keyring-backend test --home $HOME_DIR 2>/dev/null > /keys/user1.hex
yes | $BINARY keys export user2 --unarmored-hex --unsafe --keyring-backend test --home $HOME_DIR 2>/dev/null > /keys/user2.hex

# Get addresses
VALIDATOR_ADDR=$($BINARY keys show validator -a --keyring-backend test --home $HOME_DIR)
RELAYER_ADDR=$($BINARY keys show relayer -a --keyring-backend test --home $HOME_DIR)
USER1_ADDR=$($BINARY keys show user1 -a --keyring-backend test --home $HOME_DIR)
USER2_ADDR=$($BINARY keys show user2 -a --keyring-backend test --home $HOME_DIR)

# Write addresses for extraction
echo "$VALIDATOR_ADDR" > /keys/validator.addr
echo "$RELAYER_ADDR" > /keys/relayer.addr
echo "$USER1_ADDR" > /keys/user1.addr
echo "$USER2_ADDR" > /keys/user2.addr

# Fund genesis accounts
$BINARY genesis add-genesis-account $VALIDATOR_ADDR 10000000000stake --home $HOME_DIR
$BINARY genesis add-genesis-account $RELAYER_ADDR 10000000000stake --home $HOME_DIR
$BINARY genesis add-genesis-account $USER1_ADDR 10000000000stake --home $HOME_DIR
$BINARY genesis add-genesis-account $USER2_ADDR 10000000000stake --home $HOME_DIR

# Validator gentx
$BINARY genesis gentx validator 1000000000stake --chain-id $CHAIN_ID --keyring-backend test --home $HOME_DIR 2>/dev/null
$BINARY genesis collect-gentxs --home $HOME_DIR 2>/dev/null

# Set 1-second governance voting period for fast Wasm light client deployment
GENESIS_FILE="$HOME_DIR/config/genesis.json"
sed -i 's/"voting_period": *"[^"]*"/"voting_period": "10s"/g' "$GENESIS_FILE"
sed -i 's/"max_deposit_period": *"[^"]*"/"max_deposit_period": "1s"/g' "$GENESIS_FILE"

# Fast block config
sed -i 's/timeout_commit = ".*"/timeout_commit = "1s"/' $HOME_DIR/config/config.toml
sed -i 's/timeout_propose = ".*"/timeout_propose = "1s"/' $HOME_DIR/config/config.toml

# Enable gRPC
sed -i '/\[grpc\]/,/^\[/ s/enable = false/enable = true/' $HOME_DIR/config/app.toml
sed -i 's|address = "localhost:9090"|address = "0.0.0.0:9090"|' $HOME_DIR/config/app.toml

# Set gas prices
sed -i 's/minimum-gas-prices = ".*"/minimum-gas-prices = "0.00stake"/' $HOME_DIR/config/app.toml

# Keep ABCI responses so the relayer can query block results
sed -i 's/discard_abci_responses = true/discard_abci_responses = false/' $HOME_DIR/config/config.toml

exec $BINARY start --home $HOME_DIR --pruning nothing \
  --rpc.laddr tcp://0.0.0.0:{RPC_PORT} \
  --grpc.address 0.0.0.0:{GRPC_PORT}
"#
    )
}

pub struct CosmosDockerHandle {
    chain_id: String,
    rpc_endpoint: String,
    grpc_endpoint: String,
    relayer_wallet: Wallet,
    user_wallets: Vec<Wallet>,
    container: ContainerAsync<GenericImage>,
}

impl CosmosDockerHandle {
    /// Execute a shell command inside the container and return stdout.
    #[allow(clippy::future_not_send)]
    pub async fn exec_cmd(&self, cmd: &str) -> Result<String> {
        exec_in_container(&self.container, cmd).await
    }
}

impl ChainHandle for CosmosDockerHandle {
    fn rpc_endpoint(&self) -> &str {
        &self.rpc_endpoint
    }

    fn grpc_endpoint(&self) -> &str {
        &self.grpc_endpoint
    }

    fn chain_id(&self) -> &str {
        &self.chain_id
    }

    fn relayer_wallet(&self) -> &Wallet {
        &self.relayer_wallet
    }

    fn user_wallets(&self) -> &[Wallet] {
        &self.user_wallets
    }
}

#[async_trait(?Send)]
impl ChainBootstrap for CosmosDockerBootstrap {
    type Handle = CosmosDockerHandle;

    async fn start(&self) -> Result<Self::Handle> {
        let chain_id = self.generate_chain_id();
        let init_script = generate_init_script(&chain_id);

        info!(chain_id = %chain_id, "starting Cosmos chain container");

        // Override ENTRYPOINT to run our init script via sh.
        let container = GenericImage::new(IMAGE, IMAGE_TAG)
            .with_exposed_port(RPC_PORT.tcp())
            .with_exposed_port(GRPC_PORT.tcp())
            .with_entrypoint("sh")
            .with_cmd(["-c", &init_script])
            .with_startup_timeout(READINESS_TIMEOUT)
            .start()
            .await
            .wrap_err("failed to start simd container")?;

        let rpc_host_port = container
            .get_host_port_ipv4(RPC_PORT)
            .await
            .wrap_err("failed to get RPC host port")?;
        let grpc_host_port = container
            .get_host_port_ipv4(GRPC_PORT)
            .await
            .wrap_err("failed to get gRPC host port")?;

        let rpc_endpoint = format!("http://127.0.0.1:{rpc_host_port}");
        let grpc_endpoint = format!("http://127.0.0.1:{grpc_host_port}");

        poll_until_ready(&rpc_endpoint, &grpc_endpoint).await?;

        let relayer_wallet = extract_wallet(&container, "relayer").await?;
        let user1_wallet = extract_wallet(&container, "user1").await?;
        let user2_wallet = extract_wallet(&container, "user2").await?;

        info!(chain_id = %chain_id, rpc = %rpc_endpoint, "Cosmos chain ready");

        Ok(CosmosDockerHandle {
            chain_id,
            rpc_endpoint,
            grpc_endpoint,
            relayer_wallet,
            user_wallets: vec![user1_wallet, user2_wallet],
            container,
        })
    }
}

async fn poll_until_ready(rpc_endpoint: &str, grpc_endpoint: &str) -> Result<()> {
    let start = std::time::Instant::now();
    let mut warned = false;

    loop {
        let elapsed = start.elapsed();
        if elapsed > READINESS_TIMEOUT {
            eyre::bail!(
                "chain not ready after {elapsed:?} — RPC: {rpc_endpoint}, gRPC: {grpc_endpoint}"
            );
        }

        if !warned && elapsed > READINESS_WARNING_THRESHOLD {
            warn!(
                elapsed = ?elapsed,
                "chain taking longer than expected to start — expected 5-10s"
            );
            warned = true;
        }

        let rpc_ok = tendermint_rpc::HttpClient::new(rpc_endpoint)
            .ok()
            .map(|c| async move {
                use tendermint_rpc::Client;
                c.status()
                    .await
                    .is_ok_and(|s| s.sync_info.latest_block_height.value() > 0)
            });
        let rpc_ready = match rpc_ok {
            Some(fut) => fut.await,
            None => false,
        };

        let grpc_ready = tonic::transport::Channel::from_shared(grpc_endpoint.to_string())
            .ok()
            .map(|ep| async move { ep.connect().await.is_ok() });
        let grpc_ready = match grpc_ready {
            Some(fut) => fut.await,
            None => false,
        };

        if rpc_ready && grpc_ready {
            info!(elapsed = ?start.elapsed(), "chain ready");
            return Ok(());
        }

        tokio::time::sleep(READINESS_POLL_INTERVAL).await;
    }
}

#[allow(clippy::future_not_send)]
async fn extract_wallet(container: &ContainerAsync<GenericImage>, name: &str) -> Result<Wallet> {
    let hex_key = exec_in_container(container, &format!("cat /keys/{name}.hex"))
        .await
        .wrap_err_with(|| format!("failed to read {name}.hex from container"))?;
    let address = exec_in_container(container, &format!("cat /keys/{name}.addr"))
        .await
        .wrap_err_with(|| format!("failed to read {name}.addr from container"))?;

    Ok(Wallet {
        secret_key_hex: hex_key.trim().to_string(),
        address: address.trim().to_string(),
        name: name.to_string(),
    })
}

#[allow(clippy::future_not_send)]
async fn exec_in_container(container: &ContainerAsync<GenericImage>, cmd: &str) -> Result<String> {
    let mut result = container
        .exec(ExecCommand::new(["sh", "-c", cmd]))
        .await
        .wrap_err("exec failed")?;

    let stdout = result.stdout_to_vec().await.wrap_err("reading stdout")?;
    let stderr = result.stderr_to_vec().await.wrap_err("reading stderr")?;
    let exit_code = result.exit_code().await.wrap_err("waiting for exit code")?;
    if exit_code != Some(0) {
        let stderr_str = String::from_utf8_lossy(&stderr);
        let stdout_str = String::from_utf8_lossy(&stdout);
        eyre::bail!(
            "command exited with code {exit_code:?}\ncmd: {cmd}\nstdout: {stdout_str}\nstderr: {stderr_str}"
        );
    }
    String::from_utf8(stdout).wrap_err("stdout not utf8")
}

/// Poll a governance proposal until it reaches the expected status.
#[allow(clippy::future_not_send)]
async fn poll_proposal_status(
    handle: &CosmosDockerHandle,
    proposal_id: u64,
    expected_status: &str,
    timeout: Duration,
) -> Result<()> {
    let cmd = format!(
        "simd query gov proposal {proposal_id} --home /root/.simapp --output text 2>&1 \
         | {{ grep '{expected_status}' || true; }}"
    );
    let result = tokio::time::timeout(timeout, async {
        loop {
            let output = handle.exec_cmd(&cmd).await?;
            if !output.trim().is_empty() {
                return Ok::<(), eyre::Report>(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await;
    match result {
        Ok(inner) => inner,
        Err(_) => {
            bail!("proposal {proposal_id} did not reach {expected_status} within {timeout:?}")
        }
    }
}

/// Store the dummy Wasm light client on the Cosmos chain and return its SHA256 checksum.
///
/// Submits a governance proposal to store the Wasm binary, votes on it,
/// and waits for it to pass (requires 1s voting period in genesis).
#[allow(clippy::future_not_send)]
pub async fn store_dummy_wasm_light_client(handle: &CosmosDockerHandle) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read as _;

    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let wasm_gz_path = manifest_dir
        .join("../../external/solidity-ibc-eureka/e2e/interchaintestv8/wasm/cw_dummy_light_client.wasm.gz");
    let gz_bytes = std::fs::read(&wasm_gz_path)
        .wrap_err_with(|| format!("reading {}", wasm_gz_path.display()))?;

    let mut decoder = flate2::read::GzDecoder::new(&gz_bytes[..]);
    let mut wasm_bytes = Vec::new();
    decoder
        .read_to_end(&mut wasm_bytes)
        .wrap_err("decompressing wasm binary")?;

    let checksum = hex::encode(Sha256::digest(&wasm_bytes));
    info!(checksum = %checksum, "computed Wasm light client checksum");

    // Copy gzipped Wasm into the container via base64
    let wasm_b64 = base64::engine::general_purpose::STANDARD.encode(&gz_bytes);
    handle
        .exec_cmd(&format!(
            "echo '{wasm_b64}' | base64 -d > /tmp/dummy_lc.wasm.gz"
        ))
        .await?;

    let chain_id = handle.chain_id();

    handle
        .exec_cmd(&format!(
            "simd tx ibc-wasm store-code /tmp/dummy_lc.wasm.gz \
         --title 'Store dummy LC' --summary 'E2E test' \
         --deposit 10000000stake \
         --from validator --keyring-backend test --home /root/.simapp \
         --chain-id {chain_id} --gas auto --gas-adjustment 1.5 \
         --fees 0stake -y --output json"
        ))
        .await?;

    poll_proposal_status(
        handle,
        1,
        "PROPOSAL_STATUS_VOTING_PERIOD",
        Duration::from_secs(10),
    )
    .await
    .wrap_err("waiting for proposal to enter voting period")?;

    handle
        .exec_cmd(&format!(
            "simd tx gov vote 1 yes \
         --from validator --keyring-backend test --home /root/.simapp \
         --chain-id {chain_id} --fees 0stake -y --output json"
        ))
        .await?;

    poll_proposal_status(handle, 1, "PROPOSAL_STATUS_PASSED", Duration::from_secs(30))
        .await
        .wrap_err("waiting for proposal to pass")?;

    // Verify the Wasm code was stored
    let result = handle
        .exec_cmd("simd query ibc-wasm checksums --home /root/.simapp --output json")
        .await?;
    info!(result = %result.trim(), "stored Wasm checksums");

    if !result.contains(&checksum) {
        bail!("Wasm checksum {checksum} not found in stored checksums: {result}");
    }

    Ok(checksum)
}
