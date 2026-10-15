use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::registry::build_registry;

#[derive(Subcommand)]
pub enum ConfigCmd {
    Validate(ConfigValidateCmd),
}

impl ConfigCmd {
    pub fn run(self) -> eyre::Result<()> {
        match self {
            Self::Validate(cmd) => cmd.run(),
        }
    }
}

#[derive(Args)]
pub struct ConfigValidateCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
}

impl ConfigValidateCmd {
    pub fn run(self) -> eyre::Result<()> {
        let registry = build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        println!("Configuration is valid.");
        println!("  Chains: {}", cfg.chains.len());
        println!("  Relays: {}", cfg.relays.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EXAMPLE_CONFIG, err_string, write_config};

    #[test]
    fn example_config_validates() {
        ConfigCmd::Validate(ConfigValidateCmd {
            config: EXAMPLE_CONFIG.into(),
        })
        .run()
        .unwrap();
    }

    #[test]
    fn invalid_config_fails_validation() {
        let file = write_config("[[chains]]\ntype = \"near\"\n");
        let err = err_string(ConfigValidateCmd { config: file.path }.run());
        assert_eq!(err, "unsupported chain type 'near'");
    }
}
