use mercury_core::registry::ChainRegistry;

pub fn build_registry() -> ChainRegistry {
    let mut r = ChainRegistry::new();
    mercury_cosmos_counterparties::plugin::register(&mut r);
    mercury_ethereum_counterparties::plugin::register(&mut r);
    mercury_solana_counterparties::plugin::register(&mut r);
    mercury_cosmos_cosmos_relay::register(&mut r);
    mercury_cosmos_ethereum_relay::register(&mut r);
    mercury_cosmos_solana_relay::register(&mut r);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_all_shipped_chain_types() {
        let registry = build_registry();
        for chain_type in ["cosmos", "ethereum", "solana"] {
            assert_eq!(registry.chain(chain_type).unwrap().chain_type(), chain_type);
        }
    }

    #[test]
    fn registers_both_directions_of_every_relay_pair() {
        let registry = build_registry();
        for (a, b) in [
            ("cosmos", "cosmos"),
            ("cosmos", "ethereum"),
            ("cosmos", "solana"),
        ] {
            registry.pair(a, b).unwrap();
            registry.pair(b, a).unwrap();
        }
        assert!(registry.pair("ethereum", "solana").is_err());
    }

    // Minimal CometBFT `status` endpoint so `CosmosChain::new` gets past the
    // RPC checks and reaches gRPC/TLS setup.
    async fn status_rpc() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let hash = "AB".repeat(32);
                let body = serde_json::json!({
                    "jsonrpc": "2.0", "id": serde_json::Value::Null,
                    "result": {
                        "node_info": {
                            "protocol_version": {"p2p": "8", "block": "11", "app": "0"},
                            "id": "aa".repeat(20),
                            "listen_addr": "tcp://0.0.0.0:26656",
                            "network": "testchain-1",
                            "version": "0.38.12",
                            "channels": "40202122233038606100",
                            "moniker": "mock",
                            "other": {"tx_index": "on", "rpc_address": "tcp://0.0.0.0:26657"}
                        },
                        "sync_info": {
                            "latest_block_hash": hash, "latest_app_hash": hash,
                            "latest_block_height": "10",
                            "latest_block_time": "2024-01-01T00:00:00Z",
                            "earliest_block_hash": hash, "earliest_app_hash": hash,
                            "earliest_block_height": "1",
                            "earliest_block_time": "2024-01-01T00:00:00Z",
                            "catching_up": false
                        },
                        "validator_info": {
                            "address": "AA".repeat(20),
                            "pub_key": {
                                "type": "tendermint/PubKeyEd25519",
                                "value": "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo="
                            },
                            "voting_power": "10"
                        }
                    }
                })
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        url
    }

    #[tokio::test]
    async fn cosmos_connect_with_https_grpc_errors_instead_of_panicking() {
        let rpc = status_rpc().await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("key.toml"),
            format!("secret_key = \"{}\"\n", "07".repeat(32)),
        )
        .unwrap();
        let raw: toml::Table = toml::from_str(&format!(
            r#"
            type = "cosmos"
            chain_id = "testchain-1"
            rpc_addr = "{rpc}"
            grpc_addr = "https://127.0.0.1:1"
            account_prefix = "cosmos"
            key_name = "relayer"
            key_file = "key.toml"
            gas_price = {{ amount = 0.025, denom = "uatom" }}
            "#
        ))
        .unwrap();
        let path = dir.path().to_path_buf();
        let outcome = tokio::spawn(async move {
            build_registry()
                .chain("cosmos")
                .unwrap()
                .connect(&raw, &path)
                .await
                .map(|_| ())
        })
        .await;
        match outcome {
            // nothing listens on port 1: TLS setup must succeed and the connect fail
            Ok(Err(err)) => assert!(format!("{err:#}").contains("gRPC"), "{err:#}"),
            Ok(Ok(())) => panic!("connected to a closed port"),
            Err(join) => {
                let panic = join.into_panic();
                let msg = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(ToString::to_string))
                    .unwrap_or_default();
                panic!("cosmos connect panicked: {msg}");
            }
        }
    }
}
