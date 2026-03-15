use std::sync::Once;

use eyre::Result;

#[path = "cosmos_ethereum/bootstrap.rs"]
mod bootstrap;

static INIT_TRACING: Once = Once::new();

fn init_tracing() {
    INIT_TRACING.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter("info,mercury_relay=debug,mercury_ethereum=debug,mercury_e2e=debug")
            .init();
    });
}
