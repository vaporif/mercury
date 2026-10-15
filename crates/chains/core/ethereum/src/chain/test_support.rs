//! Test-only helpers: a tiny in-process HTTP server and an `EthereumChain`
//! wired to it, so RPC-backed code paths can run without a real node.

use std::sync::Arc;

use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::{EthereumChain, PayloadClient};
use crate::config::{ClientPayloadMode, EthereumChainConfig};
use crate::types::EvmChainId;

pub const ROUTER: Address = Address::repeat_byte(0x11);
pub const LIGHT_CLIENT: Address = Address::repeat_byte(0x22);
pub const CHAIN_ID: u64 = 31337;

pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

type HttpHandler = dyn Fn(&HttpRequest) -> (u16, String) + Send + Sync;

/// Spawns a minimal HTTP/1.1 server (one request per connection) and returns
/// its base URL, e.g. `http://127.0.0.1:12345`.
pub async fn spawn_http<F>(handler: F) -> String
where
    F: Fn(&HttpRequest) -> (u16, String) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handler: Arc<HttpHandler> = Arc::new(handler);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                let Some(req) = read_request(&mut sock).await else {
                    return;
                };
                let (status, body) = handler(&req);
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    format!("http://{addr}")
}

async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<HttpRequest> {
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
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let path = request_line.next()?.to_string();
    let content_length = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(HttpRequest { method, path, body })
}

/// JSON-RPC result for a single call: `Ok(result)` or `Err(message)`.
pub type RpcResult = Result<Value, String>;

/// Spawns a JSON-RPC server dispatching each call to `handler(method, params)`.
pub async fn spawn_rpc<F>(handler: F) -> String
where
    F: Fn(&str, &Value) -> RpcResult + Send + Sync + 'static,
{
    spawn_http(move |req| {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let reply = |call: &Value| {
            let id = call["id"].clone();
            let method = call["method"].as_str().unwrap_or_default();
            match handler(method, &call["params"]) {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err(msg) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32000, "message": msg}
                }),
            }
        };
        let resp = match &body {
            Value::Array(calls) => Value::Array(calls.iter().map(reply).collect()),
            call => reply(call),
        };
        (200, resp.to_string())
    })
    .await
}

pub fn test_config(rpc_addr: &str, mode: ClientPayloadMode) -> EthereumChainConfig {
    EthereumChainConfig {
        chain_name: None,
        chain_id: CHAIN_ID,
        rpc_addr: rpc_addr.to_string(),
        ws_addr: None,
        ics26_router: ROUTER.to_string(),
        key_file: "key.hex".into(),
        block_time_secs: 1,
        deployment_block: 7,
        light_client_address: Some(LIGHT_CLIENT.to_string()),
        client_payload_mode: mode,
        sp1_prover: None,
        rpc_timeout_secs: 10,
        rpc_rate_limit: 10_000,
        gas_multiplier: None,
        max_gas: None,
        max_priority_fee_multiplier: None,
    }
}

/// Builds an `EthereumChain` from `config` without contacting the node.
pub fn chain_from_config(
    config: EthereumChainConfig,
    payload_client: PayloadClient,
) -> EthereumChain {
    let provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .connect_http(config.rpc_addr.parse().unwrap())
        .erased();
    let rpc_guard =
        mercury_core::rpc_guard::RpcGuard::new(&config.chain_id.to_string(), config.rpc_config());
    EthereumChain {
        chain_id: EvmChainId(config.chain_id),
        router_address: config.router_address().unwrap(),
        provider: Arc::new(provider),
        rpc_guard,
        payload_client,
        #[cfg(feature = "sp1")]
        sp1: None,
        label: mercury_core::ChainLabel::with_id("ethereum", config.chain_id.to_string()),
        config,
    }
}

/// Chain whose RPC endpoint is `rpc_addr`, in mock payload mode.
pub fn chain_at(rpc_addr: &str) -> EthereumChain {
    chain_from_config(
        test_config(rpc_addr, ClientPayloadMode::Mock),
        PayloadClient::Mock,
    )
}

/// Chain pointing at an unroutable endpoint; for code paths that must not hit RPC.
pub fn offline_chain() -> EthereumChain {
    chain_at("http://127.0.0.1:1")
}

/// RPC handler that fails every call, for asserting no RPC is made.
pub fn no_rpc(method: &str, _: &Value) -> RpcResult {
    Err(format!("unexpected RPC call: {method}"))
}
