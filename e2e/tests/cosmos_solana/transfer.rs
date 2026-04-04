use super::*;

#[tokio::test]
#[ignore = "requires Docker and solana-test-validator"]
async fn cosmos_to_solana_transfer() -> Result<()> {
    init_tracing();

    let fixtures_dir = std::path::PathBuf::from(
        std::env::var("SOLANA_PROGRAMS_DIR")
            .map_err(|_| eyre::eyre!("SOLANA_PROGRAMS_DIR env var must be set"))?,
    );
    let _solana = mercury_e2e::bootstrap::solana::SolanaBootstrap::start(&fixtures_dir)?;

    tracing::info!("solana validator running");

    // Full relay wiring deferred — this proves the bootstrap works.
    // Steps remaining:
    // - Boot Cosmos via CosmosDockerBootstrap
    // - Build SolanaChainConfig + relay context
    // - Create client (build_create_client_payload on Cosmos, build_create_client_message on Solana counterparty)
    // - MsgTransfer on Cosmos
    // - build_update_client_message + build_receive_packet_message
    // - Assert packet receipt PDA exists

    // TODO: wire up relay context and transfer flow
    Ok(())
}
