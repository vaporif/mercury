use std::any::Any;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use mercury_chain_cache::CachedChain;
use mercury_chain_traits::builders::{ClientMessageBuilder, ClientPayloadBuilder};
use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use mercury_chain_traits::types::{ChainTypes, MessageSender};
use mercury_core::plugin::{
    self, AnyChain, ChainId, ChainPlugin, ChainStatusInfo, ClientStateInfo, DynRelayConfig,
};
use mercury_core::registry::ChainRegistry;
use mercury_relay::context::RelayWorkerConfig;
use mercury_relay::filter::PacketFilter;

use crate::builders::CosmosCreateClientPayload;
use crate::chain::CosmosChain;
use crate::client_types::CosmosClientState;
use crate::config::CosmosChainConfig;
use crate::keys::{Secp256k1KeyPair, load_cosmos_signer};
use crate::types::CosmosTxResponse;
use crate::wrapper::CosmosAdapter;

type CosmosCached = CachedChain<CosmosAdapter<Secp256k1KeyPair>>;

pub fn downcast_cosmos(chain: &AnyChain) -> eyre::Result<&CosmosCached> {
    (**chain)
        .downcast_ref::<CosmosCached>()
        .ok_or_else(|| eyre::eyre!("expected cosmos chain handle"))
}

fn parse_cosmos_client_id(
    raw: &str,
) -> eyre::Result<ibc::core::host::types::identifiers::ClientId> {
    raw.parse()
        .map_err(|e| eyre::eyre!("invalid cosmos client ID '{raw}': {e}"))
}

async fn resolve_query_params<'a>(
    chain: &'a AnyChain,
    client_id: &str,
    height: Option<u64>,
) -> eyre::Result<(
    &'a CosmosCached,
    ibc::core::host::types::identifiers::ClientId,
    tendermint::block::Height,
)> {
    let c = downcast_cosmos(chain)?;
    let parsed_id = parse_cosmos_client_id(client_id)?;
    let query_height = match height {
        Some(h) => tendermint::block::Height::try_from(h)?,
        None => c.query_latest_height().await?,
    };
    Ok((c, parsed_id, query_height))
}

fn table_to_cosmos_config(raw: &toml::Table) -> eyre::Result<CosmosChainConfig> {
    raw.clone()
        .try_into()
        .map_err(|e| eyre::eyre!("invalid cosmos config: {e}"))
}

struct CosmosPlugin;

#[async_trait]
impl ChainPlugin for CosmosPlugin {
    fn chain_type(&self) -> &'static str {
        "cosmos"
    }

    fn validate_config(&self, raw: &toml::Table) -> eyre::Result<()> {
        let cfg = table_to_cosmos_config(raw)?;
        cfg.validate()
    }

    async fn connect(&self, raw_config: &toml::Table, config_dir: &Path) -> eyre::Result<AnyChain> {
        let cfg = table_to_cosmos_config(raw_config)?;
        let key_path = config_dir.join(&cfg.key_file);

        #[cfg(unix)]
        plugin::warn_key_file_permissions(&key_path);

        let signer = load_cosmos_signer(&key_path, &cfg.account_prefix)
            .map_err(|e| eyre::eyre!("loading signer for '{}': {e}", cfg.chain_id))?;

        let expected_chain_id = cfg.chain_id.clone();
        let chain = CosmosAdapter::new(cfg, signer)
            .await
            .map_err(|e| eyre::eyre!("connecting to '{expected_chain_id}': {e}"))?;

        let on_chain_id = chain.chain_id.to_string();
        if on_chain_id != expected_chain_id {
            eyre::bail!(
                "chain_id mismatch: config says '{expected_chain_id}', node reports '{on_chain_id}'"
            );
        }

        Ok(Arc::new(CachedChain::new(chain)) as AnyChain)
    }

    fn parse_client_id(&self, raw: &str) -> eyre::Result<plugin::AnyClientId> {
        Ok(Box::new(parse_cosmos_client_id(raw)?))
    }

    async fn query_status(&self, chain: &AnyChain) -> eyre::Result<ChainStatusInfo> {
        let c = downcast_cosmos(chain)?;
        let status = c.query_chain_status().await?;
        let height = CosmosCached::chain_status_height(&status);
        let timestamp = CosmosCached::chain_status_timestamp(&status);
        let chain_id = c.chain_id();
        Ok(ChainStatusInfo {
            chain_id: ChainId::from(chain_id.to_string()),
            height: height.value(),
            timestamp: timestamp.to_string(),
        })
    }

    fn chain_id_from_config(&self, raw: &toml::Table) -> eyre::Result<ChainId> {
        raw.get("chain_id")
            .and_then(toml::Value::as_str)
            .map(ChainId::from)
            .ok_or_else(|| eyre::eyre!("missing 'chain_id' in cosmos config"))
    }

    fn rpc_addr_from_config(&self, raw: &toml::Table) -> eyre::Result<String> {
        raw.get("rpc_addr")
            .and_then(toml::Value::as_str)
            .map(String::from)
            .ok_or_else(|| eyre::eyre!("missing 'rpc_addr' in cosmos config"))
    }

    async fn build_create_client_payload(
        &self,
        chain: &AnyChain,
    ) -> eyre::Result<Box<dyn Any + Send + Sync>> {
        let c = downcast_cosmos(chain)?;
        let payload =
            ClientPayloadBuilder::<CosmosAdapter<Secp256k1KeyPair>>::build_create_client_payload(c)
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
        Ok(Box::new(payload))
    }

    async fn create_client(
        &self,
        chain: &AnyChain,
        payload: Box<dyn Any + Send + Sync>,
    ) -> eyre::Result<String> {
        let c = downcast_cosmos(chain)?;

        #[cfg(feature = "ethereum-beacon")]
        if let Some(eth_payload) =
            payload.downcast_ref::<mercury_ethereum::builders::CreateClientPayload>()
        {
            let msg = ClientMessageBuilder::<mercury_ethereum::chain::EthereumChain>::build_create_client_message(
                c,
                eth_payload.clone(),
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
            let responses = c
                .inner()
                .0
                .send_messages_with_responses(vec![msg])
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
            return Ok(extract_cosmos_client_id(&responses)?.to_string());
        }

        #[cfg(feature = "solana")]
        if let Some(solana_payload) =
            payload.downcast_ref::<mercury_solana::types::SolanaCreateClientPayload>()
        {
            let msg = ClientMessageBuilder::<mercury_solana::chain::SolanaChain>::build_create_client_message(
                c,
                solana_payload.clone(),
            )
            .await
            .map_err(|e| eyre::eyre!("{e}"))?;
            let responses = c
                .inner()
                .0
                .send_messages_with_responses(vec![msg])
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
            return Ok(extract_cosmos_client_id(&responses)?.to_string());
        }

        if let Some(cosmos_payload) = payload.downcast_ref::<CosmosCreateClientPayload>() {
            let msg =
                ClientMessageBuilder::<CosmosChain<Secp256k1KeyPair>>::build_create_client_message(
                    c,
                    cosmos_payload.clone(),
                )
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
            let responses = c
                .inner()
                .0
                .send_messages_with_responses(vec![msg])
                .await
                .map_err(|e| eyre::eyre!("{e}"))?;
            return Ok(extract_cosmos_client_id(&responses)?.to_string());
        }

        eyre::bail!("unsupported reference chain payload type for cosmos host")
    }

    async fn query_client_state_info(
        &self,
        chain: &AnyChain,
        client_id: &str,
        height: Option<u64>,
    ) -> eyre::Result<ClientStateInfo> {
        let (c, parsed_id, query_height) = resolve_query_params(chain, client_id, height).await?;

        let cs = ClientQuery::<CosmosChain<Secp256k1KeyPair>>::query_client_state(
            c,
            &parsed_id,
            &query_height,
        )
        .await?;

        Ok(match &cs {
            CosmosClientState::Tendermint(tm) => ClientStateInfo {
                client_id: client_id.to_string(),
                latest_height: tm.latest_height.revision_height(),
                trusting_period: Some(tm.trusting_period),
                frozen: tm.is_frozen(),
                client_type: "tendermint".to_string(),
                chain_id: tm.chain_id.to_string(),
            },
            // TODO: wasm frozen status not available from proto
            CosmosClientState::Wasm(wasm) => ClientStateInfo {
                client_id: client_id.to_string(),
                latest_height: wasm.latest_height.as_ref().map_or(0, |h| h.revision_height),
                trusting_period: None,
                frozen: false,
                client_type: "wasm".to_string(),
                chain_id: String::new(),
            },
            CosmosClientState::Attestations(cs) => ClientStateInfo {
                client_id: client_id.to_string(),
                latest_height: cs.latest_height,
                trusting_period: None,
                frozen: cs.is_frozen,
                client_type: "attestations".to_string(),
                chain_id: String::new(),
            },
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
        _counterparty_client_state: Option<&(dyn Any + Send + Sync)>,
    ) -> eyre::Result<Box<dyn Any + Send + Sync>> {
        let c = downcast_cosmos(chain)?;

        let trusted = tendermint::block::Height::try_from(trusted_height)?;
        let target = tendermint::block::Height::try_from(target_height)?;

        let payload =
            ClientPayloadBuilder::<CosmosAdapter<Secp256k1KeyPair>>::build_update_client_payload(
                c,
                &trusted,
                &target,
                &CosmosClientState::placeholder(),
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
        let c = downcast_cosmos(chain)?;
        let parsed_id = parse_cosmos_client_id(client_id)?;

        #[cfg(feature = "ethereum-beacon")]
        if let Some(eth_payload) =
            payload.downcast_ref::<mercury_ethereum::builders::UpdateClientPayload>()
        {
            let output =
                ClientMessageBuilder::<mercury_ethereum::chain::EthereumChain>::build_update_client_message(
                    c,
                    &parsed_id,
                    eth_payload.clone(),
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

        #[cfg(feature = "solana")]
        if let Some(solana_payload) =
            payload.downcast_ref::<mercury_solana::types::SolanaUpdateClientPayload>()
        {
            let output =
                ClientMessageBuilder::<mercury_solana::chain::SolanaChain>::build_update_client_message(
                    c,
                    &parsed_id,
                    solana_payload.clone(),
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

        if let Some(cosmos_payload) =
            payload.downcast_ref::<crate::builders::CosmosUpdateClientPayload>()
        {
            let output =
                ClientMessageBuilder::<CosmosChain<Secp256k1KeyPair>>::build_update_client_message(
                    c,
                    &parsed_id,
                    cosmos_payload.clone(),
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

        eyre::bail!("unsupported reference chain payload type for cosmos host");
    }
}

pub fn extract_cosmos_client_id(
    responses: &[CosmosTxResponse],
) -> eyre::Result<ibc::core::host::types::identifiers::ClientId> {
    for response in responses {
        for event in &response.events {
            for (key, value) in &event.attributes {
                if key == "client_id" {
                    return value
                        .parse()
                        .map_err(|e| eyre::eyre!("parse client_id: {e}"));
                }
            }
        }
    }
    eyre::bail!("client_id not found in Cosmos tx response events")
}

pub fn dyn_to_worker_config(config: &DynRelayConfig) -> eyre::Result<RelayWorkerConfig> {
    let packet_filter = config
        .packet_filter_config
        .as_ref()
        .map(|v| {
            let pfc: mercury_relay::filter::PacketFilterConfig = v
                .clone()
                .try_into()
                .map_err(|e| eyre::eyre!("invalid packet_filter config: {e}"))?;
            PacketFilter::new(&pfc)
        })
        .transpose()?;

    Ok(RelayWorkerConfig {
        lookback: config.lookback_secs.map(std::time::Duration::from_secs),
        sweep_interval: config
            .sweep_interval_secs
            .map(std::time::Duration::from_secs),
        misbehaviour_scan_interval: config
            .misbehaviour_scan_interval_secs
            .map(std::time::Duration::from_secs),
        packet_filter,
        clear_on_start: config.clear_on_start,
        clear_limit: config.clear_limit,
        excluded_sequences: config.excluded_sequences.clone(),
    })
}

pub fn register(registry: &mut ChainRegistry) {
    registry.register_chain(CosmosPlugin);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CosmosEvent;

    fn config_table() -> toml::Table {
        toml::from_str(
            r#"
            chain_id = "cosmoshub-4"
            rpc_addr = "http://localhost:26657"
            grpc_addr = "http://localhost:9090"
            account_prefix = "cosmos"
            key_name = "relayer"
            key_file = "key.toml"
            gas_price = { amount = 0.025, denom = "uatom" }
            "#,
        )
        .unwrap()
    }

    fn not_cosmos_chain() -> AnyChain {
        Arc::new(42u32)
    }

    fn response(attrs: &[(&str, &str)]) -> CosmosTxResponse {
        CosmosTxResponse {
            hash: "AB".into(),
            height: tendermint::block::Height::from(1u32),
            events: vec![CosmosEvent {
                kind: "create_client".into(),
                attributes: attrs
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            }],
        }
    }

    #[test]
    fn registers_under_cosmos_type() {
        let mut registry = ChainRegistry::new();
        register(&mut registry);
        assert_eq!(registry.chain("cosmos").unwrap().chain_type(), "cosmos");
    }

    #[test]
    fn validate_config_accepts_valid_table() {
        CosmosPlugin.validate_config(&config_table()).unwrap();
    }

    #[test]
    fn validate_config_rejects_missing_field() {
        let mut raw = config_table();
        raw.remove("grpc_addr");
        let err = CosmosPlugin.validate_config(&raw).unwrap_err();
        assert!(err.to_string().contains("invalid cosmos config"), "{err}");
    }

    #[test]
    fn validate_config_runs_semantic_validation() {
        let mut raw = config_table();
        raw.insert("rpc_addr".into(), "ftp://localhost".into());
        assert!(CosmosPlugin.validate_config(&raw).is_err());
    }

    #[test]
    fn chain_id_and_rpc_addr_from_config() {
        let raw = config_table();
        assert_eq!(
            CosmosPlugin.chain_id_from_config(&raw).unwrap(),
            ChainId::from("cosmoshub-4".to_string())
        );
        assert_eq!(
            CosmosPlugin.rpc_addr_from_config(&raw).unwrap(),
            "http://localhost:26657"
        );
    }

    #[test]
    fn chain_id_and_rpc_addr_missing_or_wrong_type() {
        let mut raw = toml::Table::new();
        assert!(CosmosPlugin.chain_id_from_config(&raw).is_err());
        assert!(CosmosPlugin.rpc_addr_from_config(&raw).is_err());
        raw.insert("chain_id".into(), 5.into());
        raw.insert("rpc_addr".into(), true.into());
        assert!(CosmosPlugin.chain_id_from_config(&raw).is_err());
        assert!(CosmosPlugin.rpc_addr_from_config(&raw).is_err());
    }

    #[test]
    fn parse_client_id_valid_and_invalid() {
        let id = CosmosPlugin.parse_client_id("07-tendermint-0").unwrap();
        let id = id
            .downcast_ref::<ibc::core::host::types::identifiers::ClientId>()
            .unwrap();
        assert_eq!(id.as_str(), "07-tendermint-0");

        let err = CosmosPlugin.parse_client_id("bad id!").unwrap_err();
        assert!(err.to_string().contains("bad id!"), "{err}");
    }

    #[test]
    fn downcast_rejects_foreign_chain_handle() {
        assert!(downcast_cosmos(&not_cosmos_chain()).is_err());
    }

    #[tokio::test]
    async fn chain_operations_reject_foreign_chain_handle() {
        let chain = not_cosmos_chain();
        let p = CosmosPlugin;
        assert!(p.query_status(&chain).await.is_err());
        assert!(p.build_create_client_payload(&chain).await.is_err());
        assert!(p.create_client(&chain, Box::new(())).await.is_err());
        assert!(
            p.query_client_state_info(&chain, "07-tendermint-0", Some(1))
                .await
                .is_err()
        );
        assert!(
            p.query_commitment_sequences(&chain, "07-tendermint-0", None)
                .await
                .is_err()
        );
        assert!(
            p.build_update_client_payload(&chain, 1, 2, None)
                .await
                .is_err()
        );
        assert!(
            p.update_client(&chain, "07-tendermint-0", Box::new(()))
                .await
                .is_err()
        );
    }

    #[test]
    fn extract_client_id_from_events() {
        let responses = vec![
            response(&[("module", "ibc")]),
            response(&[
                ("client_id", "07-tendermint-5"),
                ("client_type", "07-tendermint"),
            ]),
            response(&[("client_id", "07-tendermint-6")]),
        ];
        let id = extract_cosmos_client_id(&responses).unwrap();
        assert_eq!(id.as_str(), "07-tendermint-5", "first match wins");
    }

    #[test]
    fn extract_client_id_missing() {
        assert!(extract_cosmos_client_id(&[]).is_err());
        assert!(extract_cosmos_client_id(&[response(&[("module", "ibc")])]).is_err());
    }

    #[test]
    fn extract_client_id_invalid_value() {
        let err = extract_cosmos_client_id(&[response(&[("client_id", "x")])]).unwrap_err();
        assert!(err.to_string().contains("parse client_id"), "{err}");
    }

    fn relay_config(packet_filter: Option<toml::Value>) -> DynRelayConfig {
        DynRelayConfig {
            lookback_secs: Some(60),
            sweep_interval_secs: None,
            misbehaviour_scan_interval_secs: Some(5),
            packet_filter_config: packet_filter,
            clear_on_start: true,
            clear_limit: 7,
            excluded_sequences: vec![3, 4],
        }
    }

    #[test]
    fn worker_config_maps_durations_and_flags() {
        let cfg = dyn_to_worker_config(&relay_config(None)).unwrap();
        assert_eq!(cfg.lookback, Some(std::time::Duration::from_secs(60)));
        assert_eq!(cfg.sweep_interval, None);
        assert_eq!(
            cfg.misbehaviour_scan_interval,
            Some(std::time::Duration::from_secs(5))
        );
        assert!(cfg.packet_filter.is_none());
        assert!(cfg.clear_on_start);
        assert_eq!(cfg.clear_limit, 7);
        assert_eq!(cfg.excluded_sequences, vec![3, 4]);
    }

    #[test]
    fn worker_config_builds_packet_filter() {
        let filter: toml::Value = toml::from_str::<toml::Table>(
            r#"policy = "allow"
source_ports = ["transfer"]"#,
        )
        .unwrap()
        .into();
        let cfg = dyn_to_worker_config(&relay_config(Some(filter))).unwrap();
        let filter = cfg.packet_filter.unwrap();
        assert!(filter.allows(&[mercury_chain_traits::types::Port("transfer".into())]));
        assert!(!filter.allows(&[mercury_chain_traits::types::Port("ica".into())]));
    }

    #[test]
    fn worker_config_rejects_malformed_packet_filter() {
        let bad_shape: toml::Value = toml::from_str::<toml::Table>(r#"policy = "maybe""#)
            .unwrap()
            .into();
        let Err(err) = dyn_to_worker_config(&relay_config(Some(bad_shape))) else {
            panic!("malformed packet filter must be rejected");
        };
        assert!(err.to_string().contains("invalid packet_filter"), "{err}");

        let empty_ports: toml::Value = toml::from_str::<toml::Table>(
            r#"policy = "deny"
source_ports = []"#,
        )
        .unwrap()
        .into();
        assert!(dyn_to_worker_config(&relay_config(Some(empty_ports))).is_err());
    }
}
