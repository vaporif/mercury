use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use eyre::{ensure, Result};
use tracing::info;

static WASM_PATH: OnceLock<PathBuf> = OnceLock::new();
static MOCK_WASM_PATH: OnceLock<PathBuf> = OnceLock::new();

#[allow(clippy::missing_panics_doc)]
pub fn build_real_wasm_lc() -> &'static Path {
    WASM_PATH
        .get_or_init(|| do_build().expect("failed to build real wasm light client"))
        .as_path()
}

#[allow(clippy::missing_panics_doc)]
pub fn build_mock_wasm_lc() -> &'static Path {
    MOCK_WASM_PATH
        .get_or_init(|| do_build_mock().expect("failed to build mock wasm light client"))
        .as_path()
}

fn do_build() -> Result<PathBuf> {
    let eureka_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../external/solidity-ibc-eureka");

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

fn do_build_mock() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("MOCK_WASM_LC_PATH") {
        let p = PathBuf::from(path);
        ensure!(
            p.exists(),
            "MOCK_WASM_LC_PATH does not exist: {}",
            p.display()
        );
        info!(path = %p.display(), "using prebuilt mock wasm light client from env");
        return Ok(p);
    }

    let mock_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("mock-wasm-eth-lc");
    let artifact = mock_dir.join("mock_wasm_eth_lc.wasm.gz");

    let source_file = mock_dir.join("src/lib.rs");
    let source_is_newer = source_file
        .metadata()
        .and_then(|sm| artifact.metadata().map(|am| (sm, am)))
        .and_then(|(sm, am)| Ok(sm.modified()? > am.modified()?))
        .unwrap_or(false);

    if artifact.exists() && !source_is_newer {
        info!(path = %artifact.display(), "using prebuilt mock wasm light client");
        return Ok(artifact);
    }

    info!("building mock-wasm-eth-lc from source");

    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "--package",
            "mock-wasm-eth-lc",
        ])
        .env(
            "RUSTFLAGS",
            "-C link-arg=-s -C opt-level=z -C strip=symbols -C target-feature=-bulk-memory,-sign-ext",
        )
        .status()?;
    ensure!(status.success(), "cargo build mock-wasm-eth-lc failed");

    let wasm_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/wasm32-unknown-unknown/release/mock_wasm_eth_lc.wasm");
    ensure!(
        wasm_path.exists(),
        "wasm artifact not found: {}",
        wasm_path.display()
    );

    // Lower bulk memory ops (memory.copy/fill) to MVP wasm — wasmvm
    // doesn't support the bulk-memory proposal.
    let wasm_path_str = wasm_path.to_string_lossy();
    if Command::new("wasm-opt")
        .args([
            "--enable-bulk-memory",
            "--llvm-memory-copy-fill-lowering",
            "-Os",
            wasm_path_str.as_ref(),
            "-o",
            wasm_path_str.as_ref(),
        ])
        .status()
        .is_ok_and(|s| s.success())
    {
        info!("optimized wasm with wasm-opt");
    }

    let wasm_bytes = std::fs::read(&wasm_path)?;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    std::io::Write::write_all(&mut gz, &wasm_bytes)?;
    let gz_bytes = gz.finish()?;
    std::fs::write(&artifact, &gz_bytes)?;

    info!(path = %artifact.display(), "built mock wasm light client");
    Ok(artifact)
}
