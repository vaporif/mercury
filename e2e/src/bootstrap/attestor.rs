use std::time::Duration;

use eyre::{Context, Result};
use testcontainers::core::WaitFor;
use testcontainers::core::wait::ExitWaitStrategy;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tracing::info;

const IMAGE: &str = "ghcr.io/cosmos/ibc-attestor";
/// Same tag upstream's e2e pins; newer images require a keystore password.
const IMAGE_TAG: &str = "v0.3.0";
const KEYSTORE_DIR: &str = "/tmp/keystore";
const KEYSTORE_FILE: &str = "ibc-attestor-keystore";
const CONFIG_PATH: &str = "/tmp/attestor.toml";
const STARTUP_TIMEOUT: Duration = Duration::from_mins(1);

/// Attestor containers signing Solana state. Containers stop on drop.
pub struct SolanaAttestors {
    pub endpoints: Vec<String>,
    /// `0x`-prefixed Ethereum addresses of the signing keys.
    pub addresses: Vec<String>,
    _containers: Vec<ContainerAsync<GenericImage>>,
}

/// Starts `count` attestors for the Solana validator at `solana_rpc`.
///
/// The containers use host networking: the local validator only listens on
/// loopback, and this keeps the setup identical on Linux and macOS.
pub async fn start_solana_attestors(
    count: usize,
    solana_rpc: &str,
    router_program_id: &str,
) -> Result<SolanaAttestors> {
    let mut endpoints = Vec::with_capacity(count);
    let mut addresses = Vec::with_capacity(count);
    let mut containers = Vec::with_capacity(count);

    for _ in 0..count {
        let (address, keystore) = generate_key().await?;
        let port = free_local_port()?;
        let config = format!(
            "[server]\nlisten_addr = \"127.0.0.1:{port}\"\n\n\
             [signer]\nkeystore_path = \"{KEYSTORE_DIR}\"\n\n\
             [adapter]\nurl = \"{solana_rpc}\"\nrouter_program_id = \"{router_program_id}\"\n"
        );

        let container = GenericImage::new(IMAGE, IMAGE_TAG)
            .with_wait_for(WaitFor::message_on_stdout("gRPC server ready"))
            .with_network("host")
            .with_copy_to(format!("{KEYSTORE_DIR}/{KEYSTORE_FILE}"), keystore)
            .with_copy_to(CONFIG_PATH, config.into_bytes())
            .with_cmd(["server", "--config", CONFIG_PATH, "--chain-type", "solana"])
            .with_startup_timeout(STARTUP_TIMEOUT)
            .start()
            .await
            .wrap_err("starting attestor container")?;

        let endpoint = format!("http://127.0.0.1:{port}");
        info!(%endpoint, %address, "attestor started");
        endpoints.push(endpoint);
        addresses.push(address);
        containers.push(container);
    }

    Ok(SolanaAttestors {
        endpoints,
        addresses,
        _containers: containers,
    })
}

/// Generates a key in a one-shot container and returns its address and the
/// keystore file contents.
async fn generate_key() -> Result<(String, Vec<u8>)> {
    let script = format!(
        "ibc_attestor key generate --keystore {KEYSTORE_DIR} >/dev/null \
         && ibc_attestor key show --keystore {KEYSTORE_DIR} \
         && echo \
         && cat {KEYSTORE_DIR}/{KEYSTORE_FILE}"
    );
    let container = GenericImage::new(IMAGE, IMAGE_TAG)
        .with_entrypoint("sh")
        .with_wait_for(WaitFor::exit(ExitWaitStrategy::new().with_exit_code(0)))
        .with_cmd(["-c", script.as_str()])
        .with_startup_timeout(STARTUP_TIMEOUT)
        .start()
        .await
        .wrap_err("running attestor keygen container")?;
    let stdout = String::from_utf8(container.stdout_to_vec().await?)
        .wrap_err("attestor keygen output is not UTF-8")?;
    container.rm().await?;

    let (address, keystore) = stdout
        .split_once('\n')
        .ok_or_else(|| eyre::eyre!("unexpected attestor keygen output: {stdout}"))?;
    let address = address.trim().trim_start_matches("0x");
    eyre::ensure!(
        address.len() == 40 && address.chars().all(|c| c.is_ascii_hexdigit()),
        "unexpected attestor address: {address}"
    );
    Ok((
        format!("0x{}", address.to_lowercase()),
        keystore.trim().as_bytes().to_vec(),
    ))
}

fn free_local_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}
