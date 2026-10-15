use std::path::PathBuf;

use clap::{Args, Subcommand};
use mercury_core::plugin::SweepScope;

#[derive(Subcommand)]
pub enum ClearCmd {
    Packets(ClearPacketsCmd),
}

impl ClearCmd {
    pub async fn run(self) -> eyre::Result<()> {
        match self {
            Self::Packets(cmd) => cmd.run().await,
        }
    }
}

#[derive(Args)]
pub struct ClearPacketsCmd {
    /// Path to config file
    #[arg(short, long)]
    config: PathBuf,
    /// Source chain ID
    #[arg(long)]
    chain: String,
    /// Client ID on the source chain
    #[arg(long)]
    client: String,
    /// Counterparty chain ID
    #[arg(long)]
    counterparty_chain: String,
    /// Counterparty client ID
    #[arg(long)]
    counterparty_client: String,
    /// Specific sequences to clear (e.g. "1,5,10..20")
    #[arg(long)]
    sequences: Option<String>,
}

fn parse_sequences(input: &str) -> eyre::Result<Vec<u64>> {
    let mut result = Vec::new();
    for part in input.split(',') {
        let part = part.trim();
        if let Some((start, end)) = part.split_once("..") {
            let start: u64 = start
                .trim()
                .parse()
                .map_err(|e| eyre::eyre!("invalid sequence range start '{start}': {e}"))?;
            let end: u64 = end
                .trim()
                .parse()
                .map_err(|e| eyre::eyre!("invalid sequence range end '{end}': {e}"))?;
            if start > end {
                eyre::bail!("invalid sequence range: {start}..{end}");
            }
            result.extend(start..=end);
        } else {
            let seq: u64 = part
                .parse()
                .map_err(|e| eyre::eyre!("invalid sequence '{part}': {e}"))?;
            result.push(seq);
        }
    }
    Ok(result)
}

impl ClearPacketsCmd {
    pub async fn run(self) -> eyre::Result<()> {
        let registry = crate::registry::build_registry();
        let cfg = crate::config::load_config(&self.config, &registry)?;
        let config_dir = self
            .config
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));

        let chain_cfg = cfg.find_chain(&registry, &self.chain)?;
        let counterparty_cfg = cfg.find_chain(&registry, &self.counterparty_chain)?;

        let src_plugin = registry.chain(&chain_cfg.chain_type)?;
        let dst_plugin = registry.chain(&counterparty_cfg.chain_type)?;

        let src_chain = src_plugin.connect(&chain_cfg.raw, config_dir).await?;
        let dst_chain = dst_plugin
            .connect(&counterparty_cfg.raw, config_dir)
            .await?;

        let src_client_id = src_plugin.parse_client_id(&self.client)?;
        let dst_client_id = dst_plugin.parse_client_id(&self.counterparty_client)?;

        let pair = registry.pair(&chain_cfg.chain_type, &counterparty_cfg.chain_type)?;
        let (fwd, rev) =
            pair.build_relay(&src_chain, &dst_chain, &src_client_id, &dst_client_id)?;

        let scope = match &self.sequences {
            Some(s) => SweepScope::Sequences(parse_sequences(s)?),
            None => SweepScope::All,
        };

        eprintln!(
            "Note: if the relayer is running, some packets may already be in-flight and submissions may fail."
        );

        let (fwd_result, rev_result) =
            tokio::join!(fwd.clear_packets(scope.clone()), rev.clear_packets(scope),);

        let fwd_res = fwd_result?;
        let rev_res = rev_result?;

        println!(
            "Forward ({} -> {}):  {} recv, {} ack found",
            self.chain, self.counterparty_chain, fwd_res.recv_cleared, fwd_res.ack_cleared,
        );
        println!(
            "Reverse ({} -> {}):  {} recv, {} ack found",
            self.counterparty_chain, self.chain, rev_res.recv_cleared, rev_res.ack_cleared,
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{HUB, OSMO, err_string, example_config};

    #[test]
    fn parses_documented_sequence_syntax() {
        // Matches the `--sequences` help text example: "1,5,10..20".
        let mut expected = vec![1, 5];
        expected.extend(10..=20);
        assert_eq!(parse_sequences("1,5,10..20").unwrap(), expected);
    }

    #[test]
    fn range_bounds_are_inclusive_and_whitespace_tolerant() {
        assert_eq!(parse_sequences(" 3 .. 5 , 9 ").unwrap(), vec![3, 4, 5, 9]);
        assert_eq!(parse_sequences("7..7").unwrap(), vec![7]);
    }

    #[test]
    fn keeps_input_order_and_duplicates() {
        assert_eq!(parse_sequences("5,1,1,2..3").unwrap(), vec![5, 1, 1, 2, 3]);
    }

    #[test]
    fn rejects_reversed_range() {
        assert_eq!(
            err_string(parse_sequences("5..3")),
            "invalid sequence range: 5..3"
        );
    }

    #[test]
    fn rejects_malformed_entries() {
        for (input, prefix) in [
            ("abc", "invalid sequence 'abc'"),
            ("1,", "invalid sequence ''"),
            ("", "invalid sequence ''"),
            ("-1", "invalid sequence '-1'"),
            ("x..3", "invalid sequence range start 'x'"),
            ("1..", "invalid sequence range end ''"),
            ("1..=3", "invalid sequence range end '=3'"),
        ] {
            let err = err_string(parse_sequences(input));
            assert!(err.starts_with(prefix), "{input:?}: {err}");
        }
    }

    fn cmd(config: &std::path::Path, chain: &str, counterparty: &str) -> ClearPacketsCmd {
        ClearPacketsCmd {
            config: config.to_path_buf(),
            chain: chain.into(),
            client: "07-tendermint-0".into(),
            counterparty_chain: counterparty.into(),
            counterparty_client: "07-tendermint-0".into(),
            sequences: None,
        }
    }

    #[tokio::test]
    async fn unknown_source_chain_is_reported() {
        let cfg = example_config();
        let err = err_string(
            ClearCmd::Packets(cmd(&cfg.path, "juno-1", OSMO))
                .run()
                .await,
        );
        assert_eq!(err, "chain 'juno-1' not found in config");
    }

    #[tokio::test]
    async fn unknown_counterparty_chain_is_reported() {
        let cfg = example_config();
        let err = err_string(cmd(&cfg.path, HUB, "juno-1").run().await);
        assert_eq!(err, "chain 'juno-1' not found in config");
    }

    #[tokio::test]
    async fn connect_failure_is_reported_before_any_clearing() {
        let cfg = example_config();
        let err = err_string(cmd(&cfg.path, HUB, OSMO).run().await);
        assert!(
            err.starts_with(&format!("loading signer for '{HUB}'")),
            "{err}"
        );
    }
}
