use std::path::PathBuf;
use std::sync::Once;

#[path = "cosmos_solana/bootstrap.rs"]
mod bootstrap;

#[path = "cosmos_solana/transfer.rs"]
mod transfer;

static INIT_TRACING: Once = Once::new();

fn init_tracing() {
    INIT_TRACING.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter(
                "info,mercury_relay=debug,mercury_solana=debug,mercury_solana_counterparties=debug",
            )
            .init();
    });
}

const PROGRAMS: &[&str] = &[
    "ics26_router",
    "ics07_tendermint",
    "access_manager",
    "test_ibc_app",
];

fn solana_fixtures_dir() -> eyre::Result<PathBuf> {
    if let Ok(dir) = std::env::var("SOLANA_PROGRAMS_DIR") {
        return Ok(PathBuf::from(dir));
    }

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("e2e crate must be inside workspace")
        .to_path_buf();
    let fixtures_dir = workspace_root.join("target/solana-fixtures");

    let all_present = PROGRAMS.iter().all(|p| {
        fixtures_dir.join(format!("{p}.so")).exists()
            && fixtures_dir.join(format!("{p}-keypair.json")).exists()
    });

    if all_present {
        return Ok(fixtures_dir);
    }

    eprintln!("Solana fixtures missing — building with `anchor build`…");
    let anchor_dir = workspace_root.join("external/solidity-ibc-eureka/programs/solana");
    let status = std::process::Command::new("anchor")
        .arg("build")
        .current_dir(&anchor_dir)
        .status()
        .map_err(|e| eyre::eyre!("failed to run `anchor build`: {e}"))?;
    if !status.success() {
        eyre::bail!("`anchor build` failed with {status}");
    }

    std::fs::create_dir_all(&fixtures_dir)?;
    let deploy_dir = anchor_dir.join("target/deploy");
    for prog in PROGRAMS {
        for suffix in [".so", "-keypair.json"] {
            let name = format!("{prog}{suffix}");
            std::fs::copy(deploy_dir.join(&name), fixtures_dir.join(&name))
                .map_err(|e| eyre::eyre!("copy {name}: {e}"))?;
        }
    }

    Ok(fixtures_dir)
}
