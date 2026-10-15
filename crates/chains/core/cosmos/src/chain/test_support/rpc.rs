//! In-process `CometBFT` JSON-RPC mock. Each call is recorded and answered by a
//! handler keyed on the JSON-RPC method name.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug)]
pub struct RpcCall {
    pub method: String,
    pub params: Value,
}

pub enum Reply {
    Result(Value),
    /// JSON-RPC error object, as `CometBFT` returns for e.g. "tx not found".
    Error(String),
    /// Raw HTTP 200 body, for malformed-response tests.
    Raw(String),
    /// Never answer; the caller's timeout has to fire.
    Hang,
}

pub fn ok<T: Serialize>(value: &T) -> Reply {
    Reply::Result(serde_json::to_value(value).unwrap())
}

type Handler = dyn Fn(&RpcCall) -> Reply + Send + Sync;

pub struct MockRpc {
    pub url: String,
    calls: Arc<Mutex<Vec<RpcCall>>>,
}

impl MockRpc {
    pub fn calls(&self) -> Vec<RpcCall> {
        self.calls.lock().unwrap().clone()
    }

    pub fn calls_to(&self, method: &str) -> Vec<Value> {
        self.calls()
            .into_iter()
            .filter(|c| c.method == method)
            .map(|c| c.params)
            .collect()
    }
}

pub async fn spawn_rpc<F>(handler: F) -> MockRpc
where
    F: Fn(&RpcCall) -> Reply + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handler: Arc<Handler> = Arc::new(handler);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&calls);
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            tokio::spawn(serve(sock, Arc::clone(&handler), Arc::clone(&log)));
        }
    });
    MockRpc { url, calls }
}

async fn serve(mut sock: TcpStream, handler: Arc<Handler>, log: Arc<Mutex<Vec<RpcCall>>>) {
    let Some(body) = read_body(&mut sock).await else {
        return;
    };
    let request: Value = serde_json::from_slice(&body).unwrap();
    let call = RpcCall {
        method: request["method"].as_str().unwrap_or_default().to_string(),
        params: request["params"].clone(),
    };
    let reply = handler(&call);
    log.lock().unwrap().push(call);
    let id = request["id"].clone();
    let body = match reply {
        Reply::Result(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
        Reply::Error(data) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32603, "message": "Internal error", "data": data}
        })
        .to_string(),
        Reply::Raw(raw) => raw,
        Reply::Hang => return std::future::pending().await,
    };
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes()).await;
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
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let content_length = head
        .lines()
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
    Some(body)
}
