use std::sync::Once;

use eyre::Result;

#[path = "cosmos_ethereum/bootstrap.rs"]
mod bootstrap;
#[path = "cosmos_ethereum/context_setup.rs"]
mod context_setup;
#[path = "cosmos_ethereum/create_client.rs"]
mod create_client;
#[path = "cosmos_ethereum/transfer.rs"]
mod transfer;
#[path = "cosmos_ethereum/transfer_beacon.rs"]
mod transfer_beacon;
#[path = "cosmos_ethereum/update_client.rs"]
mod update_client;

static INIT_TRACING: Once = Once::new();

fn init_tracing() {
    INIT_TRACING.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter("info,mercury_relay=debug,mercury_ethereum=debug,mercury_e2e=debug")
            .init();
    });
}
