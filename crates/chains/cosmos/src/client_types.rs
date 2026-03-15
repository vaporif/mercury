use ibc_client_tendermint::types::ClientState as TendermintClientState;
use ibc_client_tendermint::types::ConsensusState as TendermintConsensusState;
use ibc_proto::ibc::lightclients::wasm::v1::ClientState as WasmClientState;
use ibc_proto::ibc::lightclients::wasm::v1::ConsensusState as WasmConsensusState;

/// Client state stored on a Cosmos chain. Tendermint for native IBC clients,
/// Wasm for light clients implemented as `CosmWasm` contracts (Beacon, Solana, etc.).
///
/// Cross-chain bridge impls match on this enum to extract the inner light client
/// state (e.g., `Wasm.data` contains the beacon client state bytes for Ethereum).
/// Adding a new variant will cause a compile error in all bridge crates, ensuring
/// explicit handling. See `docs/adding-a-chain.md` for the full cross-chain wiring guide.
#[derive(Clone, Debug)]
pub enum CosmosClientState {
    Tendermint(TendermintClientState),
    Wasm(WasmClientState),
}

/// Consensus state stored on a Cosmos chain.
#[derive(Clone, Debug)]
pub enum CosmosConsensusState {
    Tendermint(TendermintConsensusState),
    Wasm(WasmConsensusState),
}

pub const TENDERMINT_CLIENT_STATE_TYPE_URL: &str = "ibc.lightclients.tendermint.v1.ClientState";
pub const WASM_CLIENT_STATE_TYPE_URL: &str = "ibc.lightclients.wasm.v1.ClientState";
pub const TENDERMINT_CONSENSUS_STATE_TYPE_URL: &str =
    "ibc.lightclients.tendermint.v1.ConsensusState";
pub const WASM_CONSENSUS_STATE_TYPE_URL: &str = "ibc.lightclients.wasm.v1.ConsensusState";
