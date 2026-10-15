use ibc_client_tendermint::types::ClientState as TendermintClientState;
use ibc_client_tendermint::types::ConsensusState as TendermintConsensusState;
use ibc_proto::Protobuf;
use ibc_proto::google::protobuf::Any;
use ibc_proto::ibc::lightclients::wasm::v1::ClientState as WasmClientState;
use ibc_proto::ibc::lightclients::wasm::v1::ConsensusState as WasmConsensusState;
use mercury_attestor::proto::lightclient::{
    self as attestations, ClientState as AttestationsClientState,
    ConsensusState as AttestationsConsensusState,
};
use mercury_core::error::{QueryError, Result};
use prost::Message;

/// Client state on a Cosmos chain. Tendermint for native IBC, Wasm for
/// `CosmWasm` light clients (Beacon, Solana, etc.), Attestations for ibc-go's
/// attestor-backed client.
///
/// Adding a variant causes compile errors in all bridge crates —
/// see `docs/adding-a-chain.md`.
#[derive(Clone, Debug)]
pub enum CosmosClientState {
    Tendermint(TendermintClientState),
    Wasm(WasmClientState),
    Attestations(AttestationsClientState),
}

#[derive(Clone, Debug)]
pub enum CosmosConsensusState {
    Tendermint(TendermintConsensusState),
    Wasm(WasmConsensusState),
    Attestations(AttestationsConsensusState),
}

impl CosmosClientState {
    /// Dummy value for APIs that require a client state but don't read it.
    #[must_use]
    pub const fn placeholder() -> Self {
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

/// Whether `type_url` (without the leading `/`) names `full`, which has one.
fn is_type(type_url: &str, full: &str) -> bool {
    full.strip_prefix('/') == Some(type_url)
}

impl CosmosClientState {
    pub fn decode_any(any: &Any) -> Result<Self> {
        let type_url = any.type_url.strip_prefix('/').unwrap_or(&any.type_url);
        let value = any.value.as_slice();
        Ok(match type_url {
            TENDERMINT_CLIENT_STATE_TYPE_URL => {
                Self::Tendermint(<TendermintClientState as Protobuf<
                    ibc_client_tendermint::types::proto::v1::ClientState,
                >>::decode(value)?)
            }
            WASM_CLIENT_STATE_TYPE_URL => Self::Wasm(WasmClientState::decode(value)?),
            t if is_type(t, attestations::CLIENT_STATE_TYPE_URL) => {
                Self::Attestations(AttestationsClientState::decode(value)?)
            }
            other => {
                return Err(QueryError::UnsupportedType {
                    type_url: other.to_string(),
                }
                .into());
            }
        })
    }
}

impl CosmosConsensusState {
    pub fn decode_any(any: &Any) -> Result<Self> {
        let type_url = any.type_url.strip_prefix('/').unwrap_or(&any.type_url);
        let value = any.value.as_slice();
        Ok(match type_url {
            TENDERMINT_CONSENSUS_STATE_TYPE_URL => {
                Self::Tendermint(<TendermintConsensusState as Protobuf<
                    ibc_client_tendermint::types::proto::v1::ConsensusState,
                >>::decode(value)?)
            }
            WASM_CONSENSUS_STATE_TYPE_URL => Self::Wasm(WasmConsensusState::decode(value)?),
            t if is_type(t, attestations::CONSENSUS_STATE_TYPE_URL) => {
                Self::Attestations(AttestationsConsensusState::decode(value)?)
            }
            other => {
                return Err(QueryError::UnsupportedType {
                    type_url: other.to_string(),
                }
                .into());
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn any(type_url: &str, msg: &impl Message) -> Any {
        Any {
            type_url: type_url.to_string(),
            value: msg.encode_to_vec(),
        }
    }

    #[test]
    fn decodes_attestations_client_state() {
        let cs = AttestationsClientState {
            attestor_addresses: vec!["0xab".into()],
            min_required_sigs: 1,
            latest_height: 42,
            is_frozen: false,
        };
        let decoded = CosmosClientState::decode_any(&any(attestations::CLIENT_STATE_TYPE_URL, &cs));
        assert!(matches!(decoded, Ok(CosmosClientState::Attestations(d)) if d == cs));
    }

    #[test]
    fn decodes_attestations_consensus_state() {
        let cs = AttestationsConsensusState { timestamp: 7 };
        let decoded =
            CosmosConsensusState::decode_any(&any(attestations::CONSENSUS_STATE_TYPE_URL, &cs));
        assert!(matches!(decoded, Ok(CosmosConsensusState::Attestations(d)) if d == cs));
    }

    #[test]
    fn accepts_type_url_without_leading_slash() {
        let cs = AttestationsConsensusState { timestamp: 7 };
        let url = attestations::CONSENSUS_STATE_TYPE_URL.trim_start_matches('/');
        assert!(CosmosConsensusState::decode_any(&any(url, &cs)).is_ok());
    }

    #[test]
    fn rejects_unknown_type() {
        let cs = AttestationsConsensusState { timestamp: 7 };
        assert!(CosmosClientState::decode_any(&any("/foo.Bar", &cs)).is_err());
    }

    fn fuzz_type_url(selector: u8) -> String {
        let urls = [
            TENDERMINT_CLIENT_STATE_TYPE_URL,
            WASM_CLIENT_STATE_TYPE_URL,
            attestations::CLIENT_STATE_TYPE_URL,
            TENDERMINT_CONSENSUS_STATE_TYPE_URL,
            WASM_CONSENSUS_STATE_TYPE_URL,
            attestations::CONSENSUS_STATE_TYPE_URL,
        ];
        let url = urls[usize::from(selector) % urls.len()];
        let url = url.strip_prefix('/').unwrap_or(url);
        if selector & 0x80 == 0 {
            format!("/{url}")
        } else {
            url.to_string()
        }
    }

    #[test]
    fn fuzz_decode_any() {
        bolero::check!()
            .with_type::<(u8, Vec<u8>)>()
            .for_each(|(selector, value)| {
                let any = Any {
                    type_url: fuzz_type_url(*selector),
                    value: value.clone(),
                };
                let _ = CosmosClientState::decode_any(&any);
                let _ = CosmosConsensusState::decode_any(&any);
            });
    }
}
