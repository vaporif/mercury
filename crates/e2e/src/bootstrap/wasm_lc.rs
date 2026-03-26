use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use eyre::{Result, ensure};
use tracing::info;

static WASM_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn build_real_wasm_lc() -> &'static Path {
    WASM_PATH
        .get_or_init(|| do_build().expect("failed to build real wasm light client"))
        .as_path()
}

fn do_build() -> Result<PathBuf> {
    let eureka_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../external/solidity-ibc-eureka");

    let artifact = eureka_dir.join("e2e/interchaintestv8/wasm/cw_ics08_wasm_eth.wasm.gz");

    if artifact.exists() {
        info!(path = %artifact.display(), "using prebuilt wasm light client");
        return Ok(artifact);
    }

    info!("prebuilt artifact not found, building cw_ics08_wasm_eth via just");

    let status = Command::new("just")
        .arg("build-cw-ics08-wasm-eth")
        .current_dir(&eureka_dir)
        .status()?;

    ensure!(status.success(), "just build-cw-ics08-wasm-eth failed");
    ensure!(
        artifact.exists(),
        "artifact not found after build: {}",
        artifact.display()
    );

    info!(path = %artifact.display(), "built wasm light client");
    Ok(artifact)
}
