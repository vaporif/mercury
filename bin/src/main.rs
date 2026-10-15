#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI writes command output to the terminal"
)]

use clap::Parser;
use tracing_subscriber::EnvFilter;

mod commands;
mod config;
mod registry;
#[cfg(test)]
mod test_support;

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

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("mercury-relayer").chain(args.iter().copied()))
    }

    #[test]
    fn cli_definition_passes_clap_debug_asserts() {
        Cli::command().debug_assert();
    }

    #[test]
    fn readme_usage_examples_parse() {
        let start = parse(&["start", "--config", "relayer.toml"]).unwrap();
        assert!(start.command.is_start());
        let status = parse(&[
            "status",
            "--config",
            "relayer.toml",
            "--chain",
            "cosmoshub-4",
        ])
        .unwrap();
        assert!(!status.command.is_start());
    }

    #[test]
    fn documented_subcommands_parse() {
        // docs/feature-parity.md lists these as shipped.
        let cases: &[&[&str]] = &[
            &["health-check", "-c", "r.toml"],
            &[
                "create",
                "client",
                "-c",
                "r.toml",
                "--host-chain",
                "a",
                "--reference-chain",
                "b",
            ],
            &[
                "update",
                "client",
                "-c",
                "r.toml",
                "--host-chain",
                "a",
                "--client",
                "07-tendermint-0",
            ],
            &[
                "query", "client", "state", "-c", "r.toml", "--chain", "a", "--client", "x",
            ],
            &[
                "query",
                "packet",
                "commitments",
                "-c",
                "r.toml",
                "--chain",
                "a",
                "--client",
                "x",
            ],
            &[
                "clear",
                "packets",
                "-c",
                "r.toml",
                "--chain",
                "a",
                "--client",
                "x",
                "--counterparty-chain",
                "b",
                "--counterparty-client",
                "y",
            ],
            &["config", "validate", "-c", "r.toml"],
            &[
                "keys",
                "add",
                "-c",
                "r.toml",
                "--chain",
                "a",
                "--key-file",
                "k",
            ],
            &[
                "keys",
                "delete",
                "-c",
                "r.toml",
                "--chain",
                "a",
                "--key-name",
                "k",
            ],
            &["keys", "list", "-c", "r.toml", "--chain", "a"],
            &["keys", "balance", "-c", "r.toml", "--chain", "a"],
        ];
        for args in cases {
            let cli = parse(args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
            assert!(!cli.command.is_start(), "{args:?}");
        }
    }

    #[test]
    fn log_format_defaults_to_pretty_and_is_global() {
        let cli = parse(&["config", "validate", "-c", "r.toml"]).unwrap();
        assert!(matches!(cli.log_format, LogFormat::Pretty));
        let before = parse(&["--log-format", "json", "start", "-c", "r.toml"]).unwrap();
        assert!(matches!(before.log_format, LogFormat::Json));
        let after = parse(&["start", "-c", "r.toml", "--log-format", "json"]).unwrap();
        assert!(matches!(after.log_format, LogFormat::Json));
    }

    #[test]
    fn unknown_log_format_is_rejected() {
        let err = parse(&["--log-format", "xml", "start", "-c", "r.toml"])
            .err()
            .unwrap();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
    }

    #[test]
    fn missing_subcommand_is_rejected() {
        let err = parse(&[]).err().unwrap();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        let err = parse(&["relay"]).err().unwrap();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn commands_requiring_config_reject_missing_config() {
        for args in [
            &["start"][..],
            &["status", "--chain", "a"],
            &["health-check"],
            &["config", "validate"],
        ] {
            let err = parse(args).err().unwrap();
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::MissingRequiredArgument,
                "{args:?}"
            );
        }
    }

    #[tokio::test]
    async fn run_dispatches_config_validate() {
        let cfg = crate::test_support::example_config();
        let cli = parse(&["config", "validate", "-c", cfg.path.to_str().unwrap()]).unwrap();
        cli.command.run(cli.log_format).await.unwrap();
    }

    #[tokio::test]
    async fn run_dispatches_every_subcommand_to_its_handler() {
        // A missing config file is the first thing every handler hits, so the
        // error proves dispatch reached the right handler without a live chain.
        let missing = crate::test_support::missing_path();
        let missing = missing.to_str().unwrap();
        let cases: Vec<Vec<&str>> = vec![
            vec!["status", "-c", missing, "--chain", "a"],
            vec!["config", "validate", "-c", missing],
            vec!["health-check", "-c", missing],
            vec!["keys", "list", "-c", missing, "--chain", "a"],
            vec![
                "create",
                "client",
                "-c",
                missing,
                "--host-chain",
                "a",
                "--reference-chain",
                "b",
            ],
            vec![
                "update",
                "client",
                "-c",
                missing,
                "--host-chain",
                "a",
                "--client",
                "x",
            ],
            vec![
                "query", "client", "state", "-c", missing, "--chain", "a", "--client", "x",
            ],
            vec![
                "clear",
                "packets",
                "-c",
                missing,
                "--chain",
                "a",
                "--client",
                "x",
                "--counterparty-chain",
                "b",
                "--counterparty-client",
                "y",
            ],
            vec![
                "misbehaviour",
                "-c",
                missing,
                "--chain",
                "a",
                "--client",
                "x",
            ],
        ];
        for args in cases {
            let cli = parse(&args).unwrap();
            let err = crate::test_support::err_string(cli.command.run(cli.log_format).await);
            assert!(err.starts_with("reading config"), "{args:?}: {err}");
        }
    }
}
