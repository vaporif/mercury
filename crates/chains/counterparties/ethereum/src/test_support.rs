//! Test-only JSON-RPC mock node and helpers to connect an `EthereumAdapter` to it.

use std::sync::Arc;

use alloy::primitives::{Address, B256};
use alloy::signers::local::PrivateKeySigner;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::config::{ClientPayloadMode, EthereumChainConfig};
use crate::wrapper::EthereumAdapter;

pub const CHAIN_ID: u64 = 31337;
pub const ROUTER: Address = Address::repeat_byte(0x11);
pub const LIGHT_CLIENT: Address = Address::repeat_byte(0x22);

pub type RpcResult = Result<Value, String>;
type Handler = dyn Fn(&str, &Value) -> RpcResult + Send + Sync;

/// Spawns a JSON-RPC server. `eth_chainId` and `eth_syncing` are answered
/// automatically so `EthereumChain::new` succeeds; everything else goes to
/// `handler(method, params)`.
pub async fn spawn_node<F>(handler: F) -> String
where
    F: Fn(&str, &Value) -> RpcResult + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handler: Arc<Handler> = Arc::new(handler);
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            tokio::spawn(serve(sock, Arc::clone(&handler)));
        }
    });
    format!("http://{addr}")
}

async fn serve(mut sock: TcpStream, handler: Arc<Handler>) {
    let Some(body) = read_body(&mut sock).await else {
        return;
    };
    let request: Value = serde_json::from_slice(&body).unwrap();
    let reply = |call: &Value| {
        let method = call["method"].as_str().unwrap_or_default();
        let result = match method {
            "eth_chainId" => Ok(json!(format!("{CHAIN_ID:#x}"))),
            "eth_syncing" => Ok(json!(false)),
            _ => handler(method, &call["params"]),
        };
        match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": call["id"], "result": r}),
            Err(msg) => json!({
                "jsonrpc": "2.0",
                "id": call["id"],
                "error": {"code": -32000, "message": msg}
            }),
        }
    };
    let response = match &request {
        Value::Array(calls) => Value::Array(calls.iter().map(reply).collect()),
        call => reply(call),
    }
    .to_string();
    let http = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
        response.len()
    );
    let _ = sock.write_all(http.as_bytes()).await;
    let _ = sock.shutdown().await;
}

async fn read_body(sock: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
    let content_length = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(body)
}

pub fn no_rpc(method: &str, _: &Value) -> RpcResult {
    Err(format!("unexpected RPC call: {method}"))
}

pub fn config(rpc_addr: &str) -> EthereumChainConfig {
    EthereumChainConfig {
        chain_name: None,
        chain_id: CHAIN_ID,
        rpc_addr: rpc_addr.to_string(),
        ws_addr: None,
        ics26_router: ROUTER.to_string(),
        key_file: "key.hex".into(),
        block_time_secs: 1,
        deployment_block: 0,
        light_client_address: Some(LIGHT_CLIENT.to_string()),
        client_payload_mode: ClientPayloadMode::Mock,
        sp1_prover: None,
        rpc_timeout_secs: 10,
        rpc_rate_limit: 10_000,
        gas_multiplier: None,
        max_gas: None,
        max_priority_fee_multiplier: None,
    }
}

pub fn signer() -> PrivateKeySigner {
    PrivateKeySigner::from_bytes(&B256::repeat_byte(0x01)).unwrap()
}

pub async fn adapter_with(config: EthereumChainConfig) -> EthereumAdapter {
    EthereumAdapter::new(config, signer()).await.unwrap()
}

/// Adapter connected to a mock node served by `handler`.
pub async fn adapter<F>(handler: F) -> EthereumAdapter
where
    F: Fn(&str, &Value) -> RpcResult + Send + Sync + 'static,
{
    let url = spawn_node(handler).await;
    adapter_with(config(&url)).await
}

/// Reads `to` and calldata from an `eth_call` request.
pub fn call_target(params: &Value) -> (Address, alloy::primitives::Bytes) {
    let tx = &params[0];
    let to = tx["to"].as_str().unwrap().parse().unwrap();
    let input = tx
        .get("input")
        .or_else(|| tx.get("data"))
        .and_then(Value::as_str)
        .unwrap()
        .parse()
        .unwrap();
    (to, input)
}
