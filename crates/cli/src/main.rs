use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "mercury", about = "IBC v2 relayer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the relayer
    Start {
        /// Path to config file
        #[arg(short, long)]
        config: String,
    },
    /// Query chain status
    Status {
        /// Chain ID to query
        #[arg(short, long)]
        chain_id: String,
    },
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start { config } => {
            tracing::info!(config = %config, "starting mercury relayer");
            // TODO: load config, create chains, create relay context, run event loop
            todo!("start relayer")
        }
        Commands::Status { chain_id } => {
            tracing::info!(chain_id = %chain_id, "querying chain status");
            // TODO: create chain from config, query status, print
            todo!("query status")
        }
    }
}
