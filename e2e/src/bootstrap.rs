use std::path::Path;
use std::process::Command;
use std::sync::Once;

use tracing::info;

pub mod anvil;
pub mod cosmos_docker;
pub mod kurtosis;
pub mod traits;
pub mod wasm_lc;

static BUN_INSTALL: Once = Once::new();

#[allow(clippy::missing_panics_doc)]
pub fn install_solidity_deps(eureka_dir: &Path) {
    BUN_INSTALL.call_once(|| {
        info!("installing solidity dependencies");
        let install = Command::new("bun")
            .args(["install", "--frozen-lockfile"])
            .current_dir(eureka_dir)
            .output()
            .expect("running bun install — is bun installed?");
        assert!(
            install.status.success(),
            "bun install failed:\n{}",
            String::from_utf8_lossy(&install.stderr)
        );
    });
}
