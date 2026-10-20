use std::any::Any;
use std::path::Path;
use std::sync::Arc;

use alloy::sol_types::SolEvent;
use async_trait::async_trait;
use mercury_chain_cache::CachedChain;
use mercury_chain_traits::builders::ClientPayloadBuilder;
use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use mercury_chain_traits::types::ChainTypes;
use mercury_core::plugin::{
    self, AnyChain, AnyClientId, ChainId, ChainPlugin, ChainStatusInfo, ClientStateInfo,
};
use mercury_core::registry::ChainRegistry;

use crate::chain::EthereumChain;
use crate::config::EthereumChainConfig;
use crate::contracts::ICS26Router;
use crate::keys::load_ethereum_signer;
use crate::queries::decode_client_state;
use crate::types::{EvmClientId, EvmClientState, EvmHeight, EvmTxResponse};
use crate::wrapper::EthereumAdapter;

type EthCached = CachedChain<EthereumAdapter>;

fn downcast_eth(chain: &AnyChain) -> eyre::Result<&EthCached> {
    (**chain)
        .downcast_ref::<EthCached>()
        .ok_or_else(|| eyre::eyre!("expected ethereum chain handle"))
}

async fn resolve_query_params<'a>(
    chain: &'a AnyChain,
    client_id: &str,
    height: Option<u64>,
) -> eyre::Result<(&'a EthCached, EvmClientId, EvmHeight)> {
    let c = downcast_eth(chain)?;
    let parsed_id = EvmClientId(client_id.to_string());
    let query_height = match height {
        Some(h) => EvmHeight(h),
        None => c.query_latest_height().await?,
    };
    Ok((c, parsed_id, query_height))
}

fn table_to_eth_config(raw: &toml::Table) -> eyre::Result<EthereumChainConfig> {
    raw.clone()
        .try_into()
        .map_err(|e| eyre::eyre!("invalid ethereum config: {e}"))
}

struct EthereumPlugin;

#[async_trait]
impl ChainPlugin for EthereumPlugin {
    fn chain_type(&self) -> &'static str {
        "ethereum"
    }

    fn validate_config(&self, raw: &toml::Table) -> eyre::Result<()> {
        let cfg = table_to_eth_config(raw)?;
        cfg.validate()
    }

    async fn connect(&self, raw_config: &toml::Table, config_dir: &Path) -> eyre::Result<AnyChain> {
        let cfg = table_to_eth_config(raw_config)?;
        let key_path = config_dir.join(&cfg.key_file);

        #[cfg(unix)]
        plugin::warn_key_file_permissions(&key_path);

        let expected_chain_id = cfg.chain_id;
        let signer = load_ethereum_signer(&key_path)
            .map_err(|e| eyre::eyre!("loading signer for chain {expected_chain_id}: {e}"))?;

        let chain = EthereumAdapter::new(cfg, signer)
            .await
            .map_err(|e| eyre::eyre!("connecting to chain {expected_chain_id}: {e}"))?;

        let on_chain_id = chain.chain_id().0;
        if on_chain_id != expected_chain_id {
            eyre::bail!(
                "chain_id mismatch: config says '{expected_chain_id}', node reports '{on_chain_id}'"
            );
        }

        Ok(Arc::new(CachedChain::new(chain)) as AnyChain)
    }

    fn parse_client_id(&self, raw: &str) -> eyre::Result<AnyClientId> {
        Ok(Box::new(EvmClientId(raw.to_string())))
    }

    async fn query_status(&self, chain: &AnyChain) -> eyre::Result<ChainStatusInfo> {
        let chain = downcast_eth(chain)?;
        let status = chain.query_chain_status().await?;
        let height = EthCached::chain_status_height(&status);
        let timestamp = EthCached::chain_status_timestamp(&status);
        let chain_id = chain.chain_id();
        Ok(ChainStatusInfo {
            chain_id: ChainId::from(chain_id.0.to_string()),
            height: height.0,
            timestamp: timestamp.0.to_string(),
        })
    }

    fn chain_id_from_config(&self, raw: &toml::Table) -> eyre::Result<ChainId> {
        raw.get("chain_id")
            .and_then(toml::Value::as_integer)
            .map(|id| ChainId::from(id.to_string()))
            .ok_or_else(|| eyre::eyre!("missing 'chain_id' in ethereum config"))
    }

    fn rpc_addr_from_config(&self, raw: &toml::Table) -> eyre::Result<String> {
        raw.get("rpc_addr")
            .and_then(toml::Value::as_str)
            .map(String::from)
            .ok_or_else(|| eyre::eyre!("missing 'rpc_addr' in ethereum config"))
    }

    async fn build_create_client_payload(
        &self,
        chain: &AnyChain,
    ) -> eyre::Result<Box<dyn Any + Send + Sync>> {
        let c = downcast_eth(chain)?;
        let payload = ClientPayloadBuilder::<EthereumChain>::build_create_client_payload(c)
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
        Ok(Box::new(payload))
    }

    async fn create_client(
        &self,
        chain: &AnyChain,
        payload: Box<dyn Any + Send + Sync>,
    ) -> eyre::Result<String> {
        let c = downcast_eth(chain)?;

        if c.inner().0.config.light_client_address.is_none() {
            eyre::bail!(
                "light_client_address must be set in config to create a client on Ethereum"
            );
        }

        #[cfg(feature = "cosmos-sp1")]
        if let Some(cosmos_payload) =
            payload.downcast_ref::<mercury_cosmos::builders::CosmosCreateClientPayload>()
        {
            use mercury_chain_traits::builders::ClientMessageBuilder;
            let msg = ClientMessageBuilder::<
                mercury_cosmos::chain::CosmosChain<mercury_cosmos::keys::Secp256k1KeyPair>,
            >::build_create_client_message(c, cosmos_payload.clone())
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
            let responses = c
                .inner()
                .0
                .send_messages_with_responses(vec![msg])
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
            return extract_evm_client_id(&responses).map(|id| id.to_string());
        }

        #[cfg(not(feature = "cosmos-sp1"))]
        let _ = payload;
        eyre::bail!("unsupported reference chain payload type for ethereum host")
    }

    async fn query_client_state_info(
        &self,
        chain: &AnyChain,
        client_id: &str,
        height: Option<u64>,
    ) -> eyre::Result<ClientStateInfo> {
        let (c, parsed_id, query_height) = resolve_query_params(chain, client_id, height).await?;

        let cs =
            ClientQuery::<EthereumChain>::query_client_state(c, &parsed_id, &query_height).await?;

        let decoded = decode_client_state(&cs.0)
            .ok_or_else(|| eyre::eyre!("failed to decode client state for '{client_id}'"))?;

        Ok(ClientStateInfo {
            client_id: client_id.to_string(),
            latest_height: decoded.latestHeight.revisionHeight,
            trusting_period: Some(std::time::Duration::from_secs(u64::from(
                decoded.trustingPeriod,
            ))),
            frozen: false,
            client_type: "sp1-tendermint".to_string(),
            chain_id: decoded.chainId,
        })
    }

    async fn query_commitment_sequences(
        &self,
        chain: &AnyChain,
        client_id: &str,
        height: Option<u64>,
    ) -> eyre::Result<Vec<u64>> {
        let (c, parsed_id, query_height) = resolve_query_params(chain, client_id, height).await?;

        let sequences =
            PacketStateQuery::query_commitment_sequences(c, &parsed_id, &query_height).await?;
        Ok(sequences.into_iter().map(u64::from).collect())
    }

    async fn build_update_client_payload(
        &self,
        chain: &AnyChain,
        trusted_height: u64,
        target_height: u64,
        counterparty_client_state: Option<&(dyn Any + Send + Sync)>,
    ) -> eyre::Result<Box<dyn Any + Send + Sync>> {
        let c = downcast_eth(chain)?;
        let trusted = EvmHeight(trusted_height);
        let target = EvmHeight(target_height);

        let cs = counterparty_client_state
            .and_then(|s| s.downcast_ref::<EvmClientState>().cloned())
            .unwrap_or_else(|| EvmClientState(Vec::new()));

        let payload = ClientPayloadBuilder::<EthereumChain>::build_update_client_payload(
            c, &trusted, &target, &cs,
        )
        .await
        .map_err(|e| eyre::eyre!("{e}"))?;
        Ok(Box::new(payload))
    }

    async fn update_client(
        &self,
        chain: &AnyChain,
        client_id: &str,
        payload: Box<dyn Any + Send + Sync>,
    ) -> eyre::Result<()> {
        let c = downcast_eth(chain)?;

        #[cfg(feature = "cosmos-sp1")]
        if let Some(cosmos_payload) =
            payload.downcast_ref::<mercury_cosmos::builders::CosmosUpdateClientPayload>()
        {
            use mercury_chain_traits::builders::ClientMessageBuilder;
            use mercury_chain_traits::types::MessageSender;

            let parsed_id = EvmClientId(client_id.to_string());
            let output = ClientMessageBuilder::<
                mercury_cosmos::chain::CosmosChain<mercury_cosmos::keys::Secp256k1KeyPair>,
            >::build_update_client_message(
                c, &parsed_id, cosmos_payload.clone()
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
            c.inner()
                .0
                .send_messages(output.messages)
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
            return Ok(());
        }

        #[cfg(not(feature = "cosmos-sp1"))]
        let _ = (c, client_id, payload);
        eyre::bail!("unsupported reference chain payload type for ethereum host");
    }
}

pub fn extract_evm_client_id(responses: &[EvmTxResponse]) -> eyre::Result<EvmClientId> {
    for response in responses {
        for log in &response.logs {
            if let Ok(event) = ICS26Router::ICS02ClientAdded::decode_log_data(
                &alloy::primitives::LogData::new_unchecked(
                    log.topics.clone(),
                    log.data.clone().into(),
                ),
            ) {
                return Ok(EvmClientId(event.clientId));
            }
        }
    }
    eyre::bail!("ICS02ClientAdded event not found in EVM tx response logs")
}

pub fn register(registry: &mut ChainRegistry) {
    registry.register_chain(EthereumPlugin);
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{B256, Bytes, U256};
    use mercury_core::plugin::ChainPlugin;
    use serde_json::{Value, json};

    use super::*;
    use crate::builders::{CreateClientPayload, UpdateClientPayload};
    use crate::contracts::{IICS02ClientMsgs, sp1_ics07};
    use crate::queries::{ClientStateReturn, encode_client_state};
    use crate::test_support::{
        CHAIN_ID, LIGHT_CLIENT, ROUTER, RpcResult, adapter, adapter_with, call_target, config,
        no_rpc, spawn_node,
    };
    use crate::types::EvmEvent;

    fn table(toml_src: &str) -> toml::Table {
        toml::from_str(toml_src).unwrap()
    }

    fn valid_table(rpc: &str) -> toml::Table {
        table(&format!(
            r#"
            chain_id = {CHAIN_ID}
            rpc_addr = "{rpc}"
            ics26_router = "{ROUTER}"
            key_file = "eth.key"
            [client_payload_mode]
            type = "mock"
            "#
        ))
    }

    fn any_chain(a: EthereumAdapter) -> AnyChain {
        Arc::new(CachedChain::new(a)) as AnyChain
    }

    fn block_json(number: u64, timestamp: u64) -> Value {
        let mut block = alloy::rpc::types::Block::<alloy::rpc::types::Transaction>::default();
        block.header.inner.number = number;
        block.header.inner.timestamp = timestamp;
        serde_json::to_value(block).unwrap()
    }

    fn status_rpc(method: &str, _: &Value) -> RpcResult {
        match method {
            "eth_blockNumber" => Ok(json!("0x2a")),
            "eth_getBlockByNumber" => Ok(block_json(42, 1_700_000_000)),
            other => Err(format!("unexpected {other}")),
        }
    }

    fn client_added_event(client_id: &str) -> EvmEvent {
        let data = ICS26Router::ICS02ClientAdded {
            clientId: client_id.to_string(),
            counterpartyInfo: IICS02ClientMsgs::CounterpartyInfo {
                clientId: String::new(),
                merklePrefix: vec![],
            },
            client: LIGHT_CLIENT,
        }
        .encode_log_data();
        EvmEvent {
            address: ROUTER,
            topics: data.topics().to_vec(),
            data: data.data.to_vec(),
            block_number: crate::types::BlockNumber(1),
            tx_hash: B256::ZERO,
            log_index: crate::types::LogIndex(0),
        }
    }

    fn other_event() -> EvmEvent {
        EvmEvent {
            address: ROUTER,
            topics: vec![B256::repeat_byte(0x01)],
            data: vec![1, 2, 3],
            block_number: crate::types::BlockNumber(1),
            tx_hash: B256::ZERO,
            log_index: crate::types::LogIndex(0),
        }
    }

    fn response(logs: Vec<EvmEvent>) -> EvmTxResponse {
        EvmTxResponse {
            tx_hash: B256::ZERO,
            block_number: crate::types::BlockNumber(1),
            gas_used: crate::types::GasUsed(0),
            logs,
        }
    }

    fn sp1_client_state(height: u64) -> ClientStateReturn {
        ClientStateReturn {
            chainId: "cosmoshub-4".to_string(),
            trustLevel: sp1_ics07::IICS07TendermintMsgs::TrustThreshold {
                numerator: 1,
                denominator: 3,
            },
            latestHeight: sp1_ics07::IICS02ClientMsgs::Height {
                revisionNumber: 4,
                revisionHeight: height,
            },
            trustingPeriod: 7200,
            unbondingPeriod: 14400,
            isFrozen: false,
            zkAlgorithm: 0,
        }
    }

    #[test]
    fn extract_client_id_finds_client_added_event() {
        let responses = vec![
            response(vec![other_event()]),
            response(vec![other_event(), client_added_event("client-5")]),
        ];
        assert_eq!(
            extract_evm_client_id(&responses).unwrap(),
            EvmClientId("client-5".to_string())
        );
    }

    #[test]
    fn extract_client_id_errors_without_event() {
        assert!(extract_evm_client_id(&[]).is_err());
        let err = extract_evm_client_id(&[response(vec![other_event()])]).unwrap_err();
        assert!(err.to_string().contains("ICS02ClientAdded"), "{err}");
    }

    #[test]
    fn registers_as_ethereum() {
        let mut registry = ChainRegistry::new();
        register(&mut registry);
        assert_eq!(registry.chain("ethereum").unwrap().chain_type(), "ethereum");
        assert_eq!(EthereumPlugin.chain_type(), "ethereum");
    }

    #[test]
    fn validate_config_accepts_valid_and_rejects_invalid() {
        EthereumPlugin
            .validate_config(&valid_table("http://localhost:8545"))
            .unwrap();

        let err = EthereumPlugin
            .validate_config(&table("chain_id = 1"))
            .unwrap_err();
        assert!(err.to_string().contains("invalid ethereum config"), "{err}");

        let err = EthereumPlugin
            .validate_config(&valid_table("not a url"))
            .unwrap_err();
        assert!(format!("{err:#}").contains("rpc_addr"), "{err:#}");
    }

    #[test]
    fn chain_id_and_rpc_addr_from_config() {
        let raw = valid_table("http://node:8545");
        assert_eq!(
            EthereumPlugin.chain_id_from_config(&raw).unwrap(),
            ChainId::from(CHAIN_ID.to_string())
        );
        assert_eq!(
            EthereumPlugin.rpc_addr_from_config(&raw).unwrap(),
            "http://node:8545"
        );

        let wrong_types = table("chain_id = \"1\"\nrpc_addr = 5");
        assert!(EthereumPlugin.chain_id_from_config(&wrong_types).is_err());
        assert!(EthereumPlugin.rpc_addr_from_config(&wrong_types).is_err());
        assert!(
            EthereumPlugin
                .chain_id_from_config(&toml::Table::new())
                .is_err()
        );
        assert!(
            EthereumPlugin
                .rpc_addr_from_config(&toml::Table::new())
                .is_err()
        );
    }

    #[test]
    fn parse_client_id_wraps_raw_string() {
        let id = EthereumPlugin.parse_client_id("client-0").unwrap();
        assert_eq!(
            id.downcast_ref::<EvmClientId>(),
            Some(&EvmClientId("client-0".to_string()))
        );
    }

    #[tokio::test]
    async fn non_ethereum_handle_is_rejected() {
        let chain: AnyChain = Arc::new(42u32);
        let err = EthereumPlugin.query_status(&chain).await.unwrap_err();
        assert!(
            err.to_string().contains("expected ethereum chain handle"),
            "{err}"
        );
        assert!(
            EthereumPlugin
                .build_create_client_payload(&chain)
                .await
                .is_err()
        );
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mercury-eth-plugin-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn connect_loads_key_and_reports_status() {
        let url = spawn_node(status_rpc).await;
        let dir = temp_dir("connect");
        std::fs::write(dir.join("eth.key"), format!("0x{}", "01".repeat(32))).unwrap();

        let chain = EthereumPlugin
            .connect(&valid_table(&url), &dir)
            .await
            .unwrap();
        let status = EthereumPlugin.query_status(&chain).await.unwrap();
        assert_eq!(status.chain_id, ChainId::from(CHAIN_ID.to_string()));
        assert_eq!(status.height, 42);
        assert_eq!(status.timestamp, "1700000000");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn connect_fails_without_key_file() {
        let url = spawn_node(no_rpc).await;
        let dir = temp_dir("nokey");
        let err = EthereumPlugin
            .connect(&valid_table(&url), &dir)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("loading signer"), "{err}");
    }

    #[tokio::test]
    async fn connect_fails_on_chain_id_mismatch() {
        let url = spawn_node(no_rpc).await;
        let dir = temp_dir("mismatch");
        std::fs::write(dir.join("eth.key"), "01".repeat(32)).unwrap();
        let mut raw = valid_table(&url);
        raw.insert("chain_id".to_string(), toml::Value::Integer(1));
        let err = EthereumPlugin.connect(&raw, &dir).await.unwrap_err();
        assert!(err.to_string().contains("connecting to chain 1"), "{err}");
        assert!(err.to_string().contains("mismatch"), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn connect_rejects_invalid_config() {
        let err = EthereumPlugin
            .connect(&table("chain_id = 1"), Path::new("."))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid ethereum config"), "{err}");
    }

    fn client_state_rpc(method: &str, params: &Value) -> RpcResult {
        match method {
            "eth_blockNumber" => Ok(json!("0x64")),
            "eth_getBlockByNumber" => Ok(block_json(100, 1)),
            "eth_call" => {
                let (to, _) = call_target(params);
                if to == ROUTER {
                    Ok(json!(B256::left_padding_from(LIGHT_CLIENT.as_slice())))
                } else {
                    Ok(json!(Bytes::from(encode_client_state(&sp1_client_state(
                        77
                    )))))
                }
            }
            other => Err(format!("unexpected {other}")),
        }
    }

    #[tokio::test]
    async fn client_state_info_decodes_sp1_state() {
        let chain = any_chain(adapter(client_state_rpc).await);
        for height in [None, Some(5)] {
            let info = EthereumPlugin
                .query_client_state_info(&chain, "client-0", height)
                .await
                .unwrap();
            assert_eq!(info.client_id, "client-0");
            assert_eq!(info.latest_height, 77);
            assert_eq!(
                info.trusting_period,
                Some(std::time::Duration::from_secs(7200))
            );
            assert_eq!(info.client_type, "sp1-tendermint");
            assert_eq!(info.chain_id, "cosmoshub-4");
            assert!(!info.frozen);
        }
    }

    #[tokio::test]
    async fn client_state_info_propagates_malformed_contract_response() {
        let chain = any_chain(
            adapter(|method, params| match method {
                "eth_call" if call_target(params).0 == LIGHT_CLIENT => {
                    Ok(json!(Bytes::from_static(&[1, 2, 3])))
                }
                _ => client_state_rpc(method, params),
            })
            .await,
        );
        let err = EthereumPlugin
            .query_client_state_info(&chain, "client-0", Some(1))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("clientState() failed"), "{err}");
    }

    #[tokio::test]
    async fn commitment_sequences_returns_raw_numbers() {
        let log = |seq: u64| {
            let data = ICS26Router::SendPacket {
                clientId: alloy::primitives::keccak256("client-0"),
                sequence: U256::from(seq),
                packet: crate::contracts::IICS26RouterMsgs::Packet {
                    sequence: seq,
                    sourceClient: "client-0".to_string(),
                    destClient: "dst".to_string(),
                    timeoutTimestamp: 0,
                    payloads: vec![],
                },
            }
            .encode_log_data();
            serde_json::to_value(alloy::rpc::types::Log {
                inner: alloy::primitives::Log {
                    address: ROUTER,
                    data,
                },
                block_number: Some(1),
                ..Default::default()
            })
            .unwrap()
        };
        let logs = json!([log(3), log(1)]);
        let chain = any_chain(
            adapter(move |method, params| match method {
                "eth_getLogs" => {
                    assert_eq!(params[0]["toBlock"], json!("0x9"));
                    Ok(logs.clone())
                }
                other => Err(format!("unexpected {other}")),
            })
            .await,
        );
        let seqs = EthereumPlugin
            .query_commitment_sequences(&chain, "client-0", Some(9))
            .await
            .unwrap();
        assert_eq!(seqs, vec![1, 3]);
    }

    #[tokio::test]
    async fn create_and_update_payloads_in_mock_mode() {
        let chain = any_chain(adapter(no_rpc).await);
        let create = EthereumPlugin
            .build_create_client_payload(&chain)
            .await
            .unwrap();
        assert!(create.downcast_ref::<CreateClientPayload>().is_some());

        let update = EthereumPlugin
            .build_update_client_payload(&chain, 10, 11, None)
            .await
            .unwrap();
        let update = update.downcast_ref::<UpdateClientPayload>().unwrap();
        assert_eq!(update.headers.len(), 1);

        let err = EthereumPlugin
            .build_update_client_payload(&chain, 11, 11, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must be greater"), "{err}");
    }

    #[tokio::test]
    async fn create_client_requires_light_client_address() {
        let url = spawn_node(no_rpc).await;
        let mut cfg = config(&url);
        cfg.light_client_address = None;
        let chain = any_chain(adapter_with(cfg).await);
        let err = EthereumPlugin
            .create_client(&chain, Box::new(()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("light_client_address"), "{err}");
    }

    #[tokio::test]
    async fn create_and_update_client_reject_unknown_payloads() {
        let chain = any_chain(adapter(no_rpc).await);
        let err = EthereumPlugin
            .create_client(&chain, Box::new(42u8))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unsupported"), "{err}");

        let err = EthereumPlugin
            .update_client(&chain, "client-0", Box::new(42u8))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unsupported"), "{err}");
    }
}
