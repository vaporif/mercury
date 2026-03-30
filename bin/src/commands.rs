mod clear;
mod config_cmd;
mod create;
mod health_check;
mod keys;
mod misbehaviour;
mod query;
mod start;
mod status;
mod update;

pub use clear::*;
pub use config_cmd::*;
pub use create::*;
pub use health_check::*;
pub use keys::*;
pub use misbehaviour::*;
pub use query::*;
pub use start::*;
pub use status::*;
pub use update::*;

#[derive(clap::Subcommand)]
pub enum Commands {
    /// Start the relayer
    Start(StartCmd),

    /// Query chain status
    Status(StatusCmd),

    /// Configuration management
    #[command(subcommand)]
    Config(ConfigCmd),

    /// Check health of configured chains
    HealthCheck(HealthCheckCmd),

    /// Key management
    #[command(subcommand)]
    Keys(KeysCmd),

    /// Create IBC objects
    #[command(subcommand)]
    Create(CreateCmd),

    /// Update IBC objects
    #[command(subcommand)]
    Update(UpdateCmd),

    /// Query IBC state
    #[command(subcommand)]
    Query(QueryCmd),

    /// Clear packets
    #[command(subcommand)]
    Clear(ClearCmd),

    /// Detect and submit misbehaviour evidence
    Misbehaviour(MisbehaviourCmd),
}

impl Commands {
    pub const fn is_start(&self) -> bool {
        matches!(self, Self::Start(_))
    }

    pub async fn run(self, log_format: crate::LogFormat) -> eyre::Result<()> {
        match self {
            Self::Start(cmd) => cmd.run(log_format).await,
            Self::Status(cmd) => cmd.run().await,
            Self::Config(cmd) => cmd.run().await,
            Self::HealthCheck(cmd) => cmd.run().await,
            Self::Keys(cmd) => cmd.run().await,
            Self::Create(cmd) => cmd.run().await,
            Self::Update(cmd) => cmd.run().await,
            Self::Query(cmd) => cmd.run().await,
            Self::Clear(cmd) => cmd.run().await,
            Self::Misbehaviour(cmd) => cmd.run().await,
        }
    }
}
