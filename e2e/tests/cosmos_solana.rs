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
