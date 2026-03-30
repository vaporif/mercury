#![allow(clippy::unused_async)]

use clap::Parser;
use tracing_subscriber::EnvFilter;

mod commands;
mod config;
mod registry;

use commands::Commands;

#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
pub enum LogFormat {
    #[default]
    Pretty,
    Json,
}

#[derive(Parser)]
#[command(name = "mercury", about = "IBC v2 relayer")]
struct Cli {
    #[arg(long, global = true, default_value = "pretty")]
    log_format: LogFormat,

    #[command(subcommand)]
    command: Commands,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

    if !cli.command.is_start() {
        match cli.log_format {
            LogFormat::Pretty => {
                tracing_subscriber::fmt()
                    .with_env_filter(EnvFilter::from_default_env())
                    .with_target(false)
                    .init();
            }
            LogFormat::Json => {
                tracing_subscriber::fmt()
                    .with_env_filter(EnvFilter::from_default_env())
                    .json()
                    .init();
            }
        }
    }

    cli.command.run(cli.log_format).await
}
