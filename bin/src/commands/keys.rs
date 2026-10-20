use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum KeysCmd {
    /// Add a key for a chain
    Add(KeysAddCmd),
    /// Delete a key
    Delete(KeysDeleteCmd),
    /// List keys for a chain
    List(KeysListCmd),
    /// Query key balance
    Balance(KeysBalanceCmd),
}

impl KeysCmd {
    pub fn run(self) -> eyre::Result<()> {
        match self {
            Self::Add(cmd) => cmd.run(),
            Self::Delete(cmd) => cmd.run(),
            Self::List(cmd) => cmd.run(),
            Self::Balance(cmd) => cmd.run(),
        }
    }
}

#[derive(Args)]
pub struct KeysAddCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID
    #[arg(long)]
    chain: String,
    /// Path to raw key file
    #[arg(long)]
    key_file: PathBuf,
    /// Optional alias for the key
    #[arg(long)]
    key_name: Option<String>,
}

impl KeysAddCmd {
    pub fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let _cfg = crate::config::load_config(&self.config, &registry)?;

        eyre::bail!("keys add for chain '{}' is not implemented yet", self.chain)
    }
}

#[derive(Args)]
pub struct KeysDeleteCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID
    #[arg(long)]
    chain: String,
    /// Key name to delete
    #[arg(long)]
    key_name: String,
}

impl KeysDeleteCmd {
    pub fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let _cfg = crate::config::load_config(&self.config, &registry)?;

        eyre::bail!(
            "keys delete for chain '{}' is not implemented yet",
            self.chain
        )
    }
}

#[derive(Args)]
pub struct KeysListCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID
    #[arg(long)]
    chain: String,
}

impl KeysListCmd {
    pub fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let _cfg = crate::config::load_config(&self.config, &registry)?;

        eyre::bail!(
            "keys list for chain '{}' is not implemented yet",
            self.chain
        )
    }
}

#[derive(Args)]
pub struct KeysBalanceCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Chain ID
    #[arg(long)]
    chain: String,
    /// Token denomination (defaults to gas denom from config)
    #[arg(long)]
    denom: Option<String>,
}

impl KeysBalanceCmd {
    pub fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let _cfg = crate::config::load_config(&self.config, &registry)?;

        eyre::bail!(
            "keys balance for chain '{}' is not implemented yet",
            self.chain
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EXAMPLE_CONFIG, HUB, err_string, missing_path};

    fn all(config: &std::path::Path) -> Vec<KeysCmd> {
        vec![
            KeysCmd::Add(KeysAddCmd {
                config: config.into(),
                chain: HUB.into(),
                key_file: "k".into(),
                key_name: None,
            }),
            KeysCmd::Delete(KeysDeleteCmd {
                config: config.into(),
                chain: HUB.into(),
                key_name: "k".into(),
            }),
            KeysCmd::List(KeysListCmd {
                config: config.into(),
                chain: HUB.into(),
            }),
            KeysCmd::Balance(KeysBalanceCmd {
                config: config.into(),
                chain: HUB.into(),
                denom: None,
            }),
        ]
    }

    #[test]
    fn key_commands_report_not_implemented_for_valid_config() {
        // docs/feature-parity.md lists keys add/delete/list/balance as planned.
        let verbs = ["add", "delete", "list", "balance"];
        for (cmd, verb) in all(EXAMPLE_CONFIG.as_ref()).into_iter().zip(verbs) {
            assert_eq!(
                err_string(cmd.run()),
                format!("keys {verb} for chain '{HUB}' is not implemented yet")
            );
        }
    }

    #[test]
    fn key_commands_report_config_errors_first() {
        for cmd in all(&missing_path()) {
            assert!(err_string(cmd.run()).starts_with("reading config"));
        }
    }
}
