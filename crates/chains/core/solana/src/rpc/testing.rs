//! In-process fake of the Solana JSON-RPC endpoint for unit tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{Value, json};
use solana_client::client_error::Result as ClientResult;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::RpcClientConfig;
use solana_client::rpc_request::RpcRequest;
use solana_client::rpc_sender::{RpcSender, RpcTransportStats};
use solana_commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;

use mercury_core::rpc_guard::{RpcConfig, RpcGuard};

use super::SolanaRpcClient;

/// Answers the handful of RPC methods the crate uses. Anything unknown (or a
/// method whose canned answer is unset) yields `null`, which the client fails
/// to deserialize, so it doubles as the RPC error path.
#[derive(Clone, Default)]
pub struct FakeRpc {
    pub accounts: HashMap<Pubkey, Vec<u8>>,
    pub slot: Option<u64>,
    pub block_time: Option<i64>,
    pub blocks: HashMap<u64, Value>,
    /// Pubkeys passed to `getAccountInfo`, in call order.
    pub account_queries: Arc<Mutex<Vec<Pubkey>>>,
    /// Raw wire bytes of every transaction passed to `sendTransaction`.
    pub sent_txs: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl FakeRpc {
    pub fn with_account(mut self, address: Pubkey, data: Vec<u8>) -> Self {
        self.accounts.insert(address, data);
        self
    }

    pub fn into_client(self) -> SolanaRpcClient {
        let client = RpcClient::new_sender(
            self,
            RpcClientConfig::with_commitment(CommitmentConfig::confirmed()),
        );
        SolanaRpcClient {
            client: Arc::new(client),
            guard: RpcGuard::new(
                "solana-test",
                RpcConfig {
                    rpc_timeout: Duration::from_secs(5),
                    rate_limit: 1_000_000,
                },
            ),
        }
    }

    fn account_info(&self, params: &Value) -> Value {
        let Some(pubkey) = params[0].as_str().and_then(|s| s.parse::<Pubkey>().ok()) else {
            return Value::Null;
        };
        self.account_queries.lock().unwrap().push(pubkey);
        let value = self.accounts.get(&pubkey).map_or(Value::Null, |data| {
            json!({
                "lamports": 1_000_000,
                "data": [base64::engine::general_purpose::STANDARD.encode(data), "base64"],
                "owner": Pubkey::default().to_string(),
                "executable": false,
                "rentEpoch": 0,
                "space": data.len(),
            })
        });
        json!({ "context": { "slot": 1 }, "value": value })
    }

    fn send_transaction(&self, params: &Value) -> Value {
        let Some(wire) = params[0]
            .as_str()
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        else {
            return Value::Null;
        };
        // Wire format: compact-u16 signature count (one byte for < 128), then signatures.
        let sig = Signature::try_from(&wire[1..65]).map(|s| s.to_string());
        self.sent_txs.lock().unwrap().push(wire);
        sig.map_or(Value::Null, Value::String)
    }
}

#[async_trait]
impl RpcSender for FakeRpc {
    async fn send(&self, request: RpcRequest, params: Value) -> ClientResult<Value> {
        let ctx = |value: Value| json!({ "context": { "slot": 1 }, "value": value });
        Ok(match request {
            RpcRequest::GetAccountInfo => self.account_info(&params),
            RpcRequest::GetSlot => json!(self.slot),
            RpcRequest::GetBlockTime => json!(self.block_time),
            RpcRequest::GetBlock => params[0]
                .as_u64()
                .and_then(|slot| self.blocks.get(&slot).cloned())
                .unwrap_or(Value::Null),
            RpcRequest::GetLatestBlockhash => ctx(json!({
                "blockhash": solana_sdk::hash::Hash::new_from_array([7; 32]).to_string(),
                "lastValidBlockHeight": 100,
            })),
            RpcRequest::SendTransaction => self.send_transaction(&params),
            RpcRequest::GetSignatureStatuses => {
                let n = params[0].as_array().map_or(0, Vec::len);
                let status = json!({
                    "slot": 1,
                    "confirmations": null,
                    "err": null,
                    "status": { "Ok": null },
                    "confirmationStatus": "finalized",
                });
                ctx(Value::Array(vec![status; n]))
            }
            _ => Value::Null,
        })
    }

    fn get_transport_stats(&self) -> RpcTransportStats {
        RpcTransportStats::default()
    }

    fn url(&self) -> String {
        "fake".to_string()
    }
}

/// Anchor account bytes: discriminator followed by the borsh body.
pub fn anchor_account(struct_name: &str, body: &impl borsh::BorshSerialize) -> Vec<u8> {
    let mut data = crate::accounts::anchor_account_discriminator(struct_name).to_vec();
    borsh::to_writer(&mut data, body).unwrap();
    data
}
