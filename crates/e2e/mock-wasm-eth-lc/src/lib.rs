//! Mock WASM Ethereum light client for E2E testing.
//! Accepts any EthHeader JSON, extracts finalized slot, advances height.
//! No BLS or merkle verification.

use cosmwasm_std::{
    entry_point, to_json_binary, Binary, Deps, DepsMut, Env, MessageInfo, Response, StdError,
    StdResult,
};
use prost::Message;
use serde::{Deserialize, Serialize};

const CLIENT_STATE_KEY: &[u8] = b"clientState";

#[derive(Deserialize)]
struct Header {
    consensus_update: ConsensusUpdate,
    #[allow(dead_code)]
    trusted_slot: u64,
}

#[derive(Deserialize)]
struct ConsensusUpdate {
    finalized_header: FinalizedHeader,
}

#[derive(Deserialize)]
struct FinalizedHeader {
    beacon: BeaconBlockHeader,
    execution: ExecutionPayloadHeader,
}

#[derive(Deserialize)]
struct BeaconBlockHeader {
    slot: u64,
}

#[derive(Deserialize)]
struct ExecutionPayloadHeader {
    #[serde(default)]
    state_root: String,
    #[serde(default)]
    timestamp: u64,
    #[serde(default)]
    block_number: u64,
}

#[derive(Message)]
struct WasmClientState {
    #[prost(bytes = "vec", tag = "1")]
    data: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    checksum: Vec<u8>,
    #[prost(message, optional, tag = "3")]
    latest_height: Option<Height>,
}

#[derive(Message, Clone)]
struct Height {
    #[prost(uint64, tag = "1")]
    revision_number: u64,
    #[prost(uint64, tag = "2")]
    revision_height: u64,
}

#[derive(Message)]
struct WasmConsensusState {
    #[prost(bytes = "vec", tag = "1")]
    data: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum SudoMsg {
    UpdateState(UpdateStateMsg),
    // Remaining variants are unused stubs; payload discarded.
    UpdateStateOnMisbehaviour(serde_json::Value),
    VerifyMembership(serde_json::Value),
    VerifyNonMembership(serde_json::Value),
    MigrateClientStore(serde_json::Value),
}

#[derive(Deserialize)]
struct UpdateStateMsg {
    client_message: Binary,
}

#[derive(Serialize)]
struct UpdateStateResult {
    heights: Vec<HeightJson>,
}

#[derive(Serialize)]
struct HeightJson {
    revision_number: u64,
    revision_height: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum QueryMsg {
    // Unused stubs; payload discarded.
    VerifyClientMessage(serde_json::Value),
    CheckForMisbehaviour(serde_json::Value),
    TimestampAtHeight(serde_json::Value),
    Status(serde_json::Value),
}

#[derive(Serialize)]
struct StatusResult {
    status: String,
}

#[derive(Deserialize)]
pub struct InstantiateMsg {
    client_state: Binary,
    consensus_state: Binary,
    checksum: Binary,
}

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> StdResult<Response> {
    let inner: serde_json::Value = serde_json::from_slice(&msg.client_state)
        .map_err(|e| StdError::generic_err(e.to_string()))?;
    let latest_slot = inner
        .get("latest_slot")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let wasm_cs = WasmClientState {
        data: msg.client_state.to_vec(),
        checksum: msg.checksum.to_vec(),
        latest_height: Some(Height {
            revision_number: 0,
            revision_height: latest_slot,
        }),
    };
    deps.storage.set(CLIENT_STATE_KEY, &wasm_cs.encode_to_vec());

    let wasm_cons = WasmConsensusState {
        data: msg.consensus_state.to_vec(),
    };
    let cons_key = format!("consensusStates/0-{latest_slot}");
    deps.storage
        .set(cons_key.as_bytes(), &wasm_cons.encode_to_vec());

    Ok(Response::new())
}

#[entry_point]
pub fn sudo(deps: DepsMut, _env: Env, msg: Binary) -> StdResult<Response> {
    let sudo_msg: SudoMsg =
        serde_json::from_slice(&msg).map_err(|e| StdError::generic_err(e.to_string()))?;

    match sudo_msg {
        SudoMsg::UpdateState(update_msg) => handle_update_state(deps, update_msg),
        SudoMsg::VerifyMembership(_) | SudoMsg::VerifyNonMembership(_) => {
            Ok(Response::new().set_data(Binary::default()))
        }
        _ => Ok(Response::new()),
    }
}

fn handle_update_state(deps: DepsMut, msg: UpdateStateMsg) -> StdResult<Response> {
    let header_bz: Vec<u8> = msg.client_message.into();
    let header: Header =
        serde_json::from_slice(&header_bz).map_err(|e| StdError::generic_err(e.to_string()))?;

    let updated_slot = header.consensus_update.finalized_header.beacon.slot;

    let cs_bytes = deps
        .storage
        .get(CLIENT_STATE_KEY)
        .ok_or_else(|| StdError::generic_err("client state not found"))?;
    let mut wasm_cs = WasmClientState::decode(cs_bytes.as_slice())
        .map_err(|e| StdError::generic_err(e.to_string()))?;

    let mut inner: serde_json::Value =
        serde_json::from_slice(&wasm_cs.data).map_err(|e| StdError::generic_err(e.to_string()))?;
    inner["latest_slot"] = serde_json::json!(updated_slot);
    inner["latest_execution_block_number"] = serde_json::json!(
        header
            .consensus_update
            .finalized_header
            .execution
            .block_number
    );
    wasm_cs.data = serde_json::to_vec(&inner).map_err(|e| StdError::generic_err(e.to_string()))?;

    wasm_cs.latest_height = Some(Height {
        revision_number: 0,
        revision_height: updated_slot,
    });
    deps.storage.set(CLIENT_STATE_KEY, &wasm_cs.encode_to_vec());

    let mock_consensus = serde_json::json!({
        "slot": updated_slot,
        "state_root": header.consensus_update.finalized_header.execution.state_root,
        "timestamp": header.consensus_update.finalized_header.execution.timestamp,
        "current_sync_committee": {
            "pubkeys_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "aggregate_pubkey": "0x000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
        },
        "next_sync_committee": null
    });
    let cons_bz =
        serde_json::to_vec(&mock_consensus).map_err(|e| StdError::generic_err(e.to_string()))?;
    let wasm_cons = WasmConsensusState { data: cons_bz };
    let cons_key = format!("consensusStates/0-{updated_slot}");
    deps.storage
        .set(cons_key.as_bytes(), &wasm_cons.encode_to_vec());

    let result = UpdateStateResult {
        heights: vec![HeightJson {
            revision_number: 0,
            revision_height: updated_slot,
        }],
    };
    let result_bz = to_json_binary(&result)?;
    Ok(Response::new().set_data(result_bz))
}

#[entry_point]
pub fn query(_deps: Deps, _env: Env, msg: Binary) -> StdResult<Binary> {
    let query_msg: QueryMsg =
        serde_json::from_slice(&msg).map_err(|e| StdError::generic_err(e.to_string()))?;

    match query_msg {
        QueryMsg::VerifyClientMessage(_) => Ok(Binary::default()),
        QueryMsg::CheckForMisbehaviour(_) => to_json_binary(&false),
        QueryMsg::Status(_) => to_json_binary(&StatusResult {
            status: "Active".to_string(),
        }),
        QueryMsg::TimestampAtHeight(_) => to_json_binary(&0u64),
    }
}
