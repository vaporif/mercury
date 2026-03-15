use std::process::{Child, Command, Stdio};
use std::time::Duration;

use alloy::primitives::Address;
use eyre::{Context, Result, bail};
use tracing::info;

use super::install_solidity_deps;

/// Wallet for Anvil accounts.
#[derive(Clone, Debug)]
pub struct AnvilWallet {
    pub private_key: String,
    pub address: Address,
}

/// Handle to a running Anvil instance with deployed IBC contracts.
pub struct AnvilHandle {
    child: Child,
    pub rpc_endpoint: String,
    pub chain_id: u64,
    pub ics26_router: Address,
    pub ics20_transfer: Address,
    pub light_client: Address,
    pub mock_verifier: Address,
    pub erc20: Address,
    pub relayer_wallet: AnvilWallet,
    pub user_wallets: Vec<AnvilWallet>,
}

impl Drop for AnvilHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl AnvilHandle {
    #[must_use]
    pub fn rpc_endpoint(&self) -> &str {
        &self.rpc_endpoint
    }

    #[must_use]
    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }
}

/// Start Anvil and deploy IBC contracts.
pub async fn start_anvil() -> Result<AnvilHandle> {
    let port = find_free_port()?;
    let chain_id = u64::from(port);
    let rpc_endpoint = format!("http://127.0.0.1:{port}");

    info!(port, chain_id, "starting Anvil");
    let child = Command::new("anvil")
        .args([
            "--port",
            &port.to_string(),
            "--chain-id",
            &chain_id.to_string(),
            "--block-time",
            "1",
            "--silent",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .wrap_err("spawning anvil — is foundry installed?")?;

    poll_anvil_ready(&rpc_endpoint).await?;

    let mut handle = AnvilHandle {
        child,
        rpc_endpoint: rpc_endpoint.clone(),
        chain_id,
        ics26_router: Address::ZERO,
        ics20_transfer: Address::ZERO,
        light_client: Address::ZERO,
        mock_verifier: Address::ZERO,
        erc20: Address::ZERO,
        relayer_wallet: anvil_wallet(0),
        user_wallets: vec![anvil_wallet(1), anvil_wallet(2)],
    };

    deploy_contracts(&mut handle)?;

    info!(
        rpc = %handle.rpc_endpoint,
        chain_id = handle.chain_id,
        router = %handle.ics26_router,
        transfer = %handle.ics20_transfer,
        "Anvil ready with deployed contracts"
    );

    Ok(handle)
}

fn find_free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").wrap_err("binding to free port")?;
    Ok(listener.local_addr()?.port())
}

/// Anvil's pre-funded accounts (deterministic from mnemonic).
/// These are well-known deterministic keys — safe to hardcode for tests.
fn anvil_wallet(index: u8) -> AnvilWallet {
    let (key, addr) = match index {
        0 => (
            "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            "f39Fd6e51aad88F6F4ce6aB8827279cffFb92266",
        ),
        1 => (
            "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
            "70997970C51812dc3A010C7d01b50e0d17dc79C8",
        ),
        2 => (
            "5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
            "3C44CdDdB6a900fa2b585dd299e03d12FA4293BC",
        ),
        _ => panic!("only 3 anvil wallets configured"),
    };
    AnvilWallet {
        private_key: key.to_string(),
        address: addr.parse().expect("valid anvil address"),
    }
}

async fn poll_anvil_ready(rpc_endpoint: &str) -> Result<()> {
    let timeout = Duration::from_secs(15);
    let start = std::time::Instant::now();

    loop {
        if start.elapsed() > timeout {
            bail!("Anvil not ready after {timeout:?}");
        }

        let ok = reqwest::Client::new()
            .post(rpc_endpoint)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_chainId",
                "params": [],
                "id": 1,
            }))
            .send()
            .await
            .is_ok();

        if ok {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn deploy_contracts(handle: &mut AnvilHandle) -> Result<()> {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let eureka_dir = manifest_dir.join("../../external/solidity-ibc-eureka");
    if !eureka_dir.exists() {
        bail!("external/solidity-ibc-eureka not found — did you init submodules?");
    }

    install_solidity_deps(&eureka_dir);

    let deployer = &handle.relayer_wallet;

    // Use a per-instance cache path so parallel forge runs don't corrupt shared cache.
    let cache_dir = tempfile::tempdir().wrap_err("creating forge cache dir")?;

    info!("deploying IBC contracts via forge script");
    let output = Command::new("forge")
        .args([
            "script",
            "scripts/E2ETestDeploy.s.sol",
            "--rpc-url",
            &handle.rpc_endpoint,
            "--broadcast",
            "--sender",
            &format!("{:#x}", deployer.address),
            "--unlocked",
        ])
        .current_dir(&eureka_dir)
        .env("E2E_FAUCET_ADDRESS", format!("{:#x}", deployer.address))
        .env("FOUNDRY_CACHE_PATH", cache_dir.path())
        .output()
        .wrap_err("running forge script")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("forge script failed:\n{stderr}");
    }

    // Parse deployed addresses from broadcast artifacts
    let broadcast_path = eureka_dir
        .join("broadcast")
        .join("E2ETestDeploy.s.sol")
        .join(handle.chain_id.to_string())
        .join("run-latest.json");

    let broadcast_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&broadcast_path).wrap_err_with(|| {
            format!(
                "reading broadcast artifacts at {}",
                broadcast_path.display()
            )
        })?)?;

    let addresses = parse_broadcast_returns(&broadcast_json)?;

    handle.ics26_router = addresses.ics26_router;
    handle.ics20_transfer = addresses.ics20_transfer;
    handle.mock_verifier = addresses.mock_verifier;
    handle.erc20 = addresses.erc20;

    // SP1ICS07Tendermint light client is not part of E2ETestDeploy.s.sol.
    handle.light_client = Address::ZERO;

    Ok(())
}

struct DeployedAddresses {
    ics26_router: Address,
    ics20_transfer: Address,
    mock_verifier: Address,
    erc20: Address,
}

fn extract_addr(v: &serde_json::Value, key: &str) -> Result<Address> {
    v.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("{key} not found in deploy output"))?
        .parse()
        .map_err(|e| eyre::eyre!("parsing {key} address: {e}"))
}

fn parse_broadcast_returns(broadcast: &serde_json::Value) -> Result<DeployedAddresses> {
    let returns = broadcast.get("returns").ok_or_else(|| {
        eyre::eyre!("no returns field in broadcast JSON — inspect run-latest.json manually")
    })?;

    let return_str = returns
        .get("0")
        .and_then(|v| v.get("value"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("could not extract return value from broadcast JSON"))?;

    // Forge's stdJson.serialize double-escapes the JSON string — unescape before parsing.
    let unescaped = return_str.replace("\\\"", "\"");
    let addrs: serde_json::Value =
        serde_json::from_str(&unescaped).wrap_err("parsing returned JSON from forge script")?;

    Ok(DeployedAddresses {
        ics26_router: extract_addr(&addrs, "ics26Router")?,
        ics20_transfer: extract_addr(&addrs, "ics20Transfer")?,
        mock_verifier: extract_addr(&addrs, "verifierMock")?,
        erc20: extract_addr(&addrs, "erc20")?,
    })
}
