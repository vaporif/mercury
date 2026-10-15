//! Offline `SolanaChain` construction for unit tests in dependent crates.

use std::sync::Arc;
use std::time::Duration;

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::keypair::Keypair;

use super::SolanaChain;
use crate::config::SolanaChainConfig;
use crate::rpc::testing::FakeRpc;

#[must_use]
pub fn test_config() -> SolanaChainConfig {
    SolanaChainConfig {
        rpc_addr: "http://127.0.0.1:1".into(),
        ws_addr: None,
        program_id: Pubkey::new_unique().to_string(),
        ics07_program_id: None,
        keypair_path: "/unused".into(),
        block_time: Duration::from_millis(400),
        rpc_timeout_secs: 5,
        rpc_rate_limit: 100,
        alt_address: None,
        skip_pre_verify_threshold: None,
        attestation: None,
    }
}

/// A chain that answers every RPC call from `rpc`, with a fresh payer keypair.
#[must_use]
pub fn test_chain(ics26_program_id: Pubkey, rpc: FakeRpc) -> SolanaChain {
    SolanaChain {
        config: SolanaChainConfig {
            program_id: ics26_program_id.to_string(),
            ..test_config()
        },
        rpc: rpc.into_client(),
        keypair: Arc::new(Keypair::new()),
        ics26_program_id,
        ics07_program_id: None,
        alt_cache: None,
        attestor: None,
        label: mercury_core::ChainLabel::new("solana"),
    }
}
