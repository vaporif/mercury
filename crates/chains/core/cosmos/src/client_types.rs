use ibc_client_tendermint::types::ClientState as TendermintClientState;
use ibc_client_tendermint::types::ConsensusState as TendermintConsensusState;
use ibc_proto::ibc::lightclients::wasm::v1::ClientState as WasmClientState;
use ibc_proto::ibc::lightclients::wasm::v1::ConsensusState as WasmConsensusState;

/// Client state on a Cosmos chain. Tendermint for native IBC, Wasm for
/// `CosmWasm` light clients (Beacon, Solana, etc.).
///
/// Adding a variant causes compile errors in all bridge crates —
/// see `docs/adding-a-chain.md`.
#[derive(Clone, Debug)]
pub enum CosmosClientState {
    Tendermint(TendermintClientState),
    Wasm(WasmClientState),
}

#[derive(Clone, Debug)]
pub enum CosmosConsensusState {
    Tendermint(TendermintConsensusState),
    Wasm(WasmConsensusState),
}

impl CosmosClientState {
    /// Dummy value for APIs that require a client state but don't read it.
    pub fn placeholder() -> Self {
        Self::Wasm(ibc_proto::ibc::lightclients::wasm::v1::ClientState {
            data: Vec::new(),
            checksum: Vec::new(),
            latest_height: None,
        })
    }
}

pub const TENDERMINT_CLIENT_STATE_TYPE_URL: &str = "ibc.lightclients.tendermint.v1.ClientState";
pub const WASM_CLIENT_STATE_TYPE_URL: &str = "ibc.lightclients.wasm.v1.ClientState";
pub const TENDERMINT_CONSENSUS_STATE_TYPE_URL: &str =
    "ibc.lightclients.tendermint.v1.ConsensusState";
pub const WASM_CONSENSUS_STATE_TYPE_URL: &str = "ibc.lightclients.wasm.v1.ConsensusState";
