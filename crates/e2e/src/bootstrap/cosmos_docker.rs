use std::time::Duration;

use async_trait::async_trait;
use eyre::{Context, Result};
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
    let exit_code = result.exit_code().await.wrap_err("waiting for exit code")?;
    eyre::ensure!(
        exit_code == Some(0),
        "command exited with code {exit_code:?}"
    );
    String::from_utf8(stdout).wrap_err("stdout not utf8")
}
