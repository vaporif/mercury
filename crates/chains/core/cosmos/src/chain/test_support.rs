//! Offline `CosmosChain` construction for unit tests. Endpoints point at a
//! closed local port, so any code path that reaches the network fails fast.

use std::sync::{Arc, OnceLock};

use ibc::core::host::types::identifiers::ChainId;
use tendermint_rpc::HttpClient;

use super::CosmosChain;
use crate::config::CosmosChainConfig;
use crate::keys::Secp256k1KeyPair;

pub const UNREACHABLE_ADDR: &str = "http://127.0.0.1:1";

pub fn test_config() -> CosmosChainConfig {
    toml::from_str(&format!(
        r#"
        chain_id = "testchain-1"
        rpc_addr = "{UNREACHABLE_ADDR}"
        grpc_addr = "{UNREACHABLE_ADDR}"
        account_prefix = "cosmos"
        key_name = "test"
        key_file = "key.toml"
        rpc_timeout_secs = 5
        gas_price = {{ amount = 0.025, denom = "uatom" }}
        "#
    ))
    .unwrap()
}

pub fn test_signer() -> Secp256k1KeyPair {
    let secret = secp256k1::SecretKey::from_secret_bytes([7u8; 32]).unwrap();
    Secp256k1KeyPair::from_secret_key(secret, "cosmos")
}

/// Must be called from within a tokio runtime (the lazy gRPC channel spawns a worker).
pub fn test_chain_with(config: CosmosChainConfig) -> CosmosChain<Secp256k1KeyPair> {
    let rpc_client = HttpClient::new(config.rpc_addr.as_str()).unwrap();
    let grpc_channel = tonic::transport::Channel::from_shared(config.grpc_addr.clone())
        .unwrap()
        .connect_lazy();
    let chain_id = ChainId::new(&config.chain_id).unwrap();
    let rpc_guard = mercury_core::rpc_guard::RpcGuard::new(&config.chain_id, config.rpc_config());
    let label = mercury_core::ChainLabel::with_id("cosmos", chain_id.to_string());
    let gas_metrics = mercury_telemetry::recorder::GasMetrics::new(&label);
    CosmosChain {
        block_time: config.block_time,
        config,
        chain_id,
        rpc_client,
        grpc_channel,
        signer: test_signer(),
        rpc_guard,
        dynamic_gas_backend: Arc::new(OnceLock::new()),
        gas_metrics,
        label,
    }
}

pub fn test_chain() -> CosmosChain<Secp256k1KeyPair> {
    test_chain_with(test_config())
}

pub fn tm_client_state(latest_height: u64) -> ibc_client_tendermint::types::ClientState {
    use ibc::core::client::types::Height;
    use ibc::core::commitment_types::specs::ProofSpecs;
    use ibc_client_tendermint::types::{AllowUpdate, ClientState, TrustThreshold};
    use std::time::Duration;

    ClientState::new(
        ChainId::new("counterparty-1").unwrap(),
        TrustThreshold::ONE_THIRD,
        Duration::from_secs(1000),
        Duration::from_secs(2000),
        Duration::from_secs(10),
        Height::new(1, latest_height).unwrap(),
        ProofSpecs::cosmos(),
        vec!["upgrade".to_string()],
        AllowUpdate {
            after_expiry: true,
            after_misbehaviour: true,
        },
    )
    .unwrap()
}

/// Unsigned header at `height`; `app_hash_byte` distinguishes conflicting headers.
pub fn tm_ibc_header(height: u64, app_hash_byte: u8) -> ibc_client_tendermint::types::Header {
    use ibc::core::client::types::Height;
    use tendermint::block::{Commit, Header, Height as TmHeight, signed_header::SignedHeader};
    use tendermint::validator::Set as ValidatorSet;

    let tm_height = TmHeight::try_from(height).unwrap();
    let header = Header {
        version: tendermint::block::header::Version { block: 11, app: 0 },
        chain_id: "testchain-1".parse().unwrap(),
        height: tm_height,
        time: tendermint::Time::from_unix_timestamp(1_700_000_000, 0).unwrap(),
        last_block_id: None,
        last_commit_hash: None,
        data_hash: None,
        validators_hash: tendermint::Hash::None,
        next_validators_hash: tendermint::Hash::None,
        consensus_hash: tendermint::Hash::None,
        app_hash: tendermint::AppHash::try_from(vec![app_hash_byte; 32]).unwrap(),
        last_results_hash: None,
        evidence_hash: None,
        proposer_address: tendermint::account::Id::new([0u8; 20]),
    };
    let commit = Commit {
        height: tm_height,
        ..Commit::default()
    };
    ibc_client_tendermint::types::Header {
        signed_header: SignedHeader::new(header, commit).unwrap(),
        validator_set: ValidatorSet::new(vec![], None),
        trusted_height: Height::new(1, height - 1).unwrap(),
        trusted_next_validator_set: ValidatorSet::new(vec![], None),
    }
}
