use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use mercury_chain_cache::CachedChain;
use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use mercury_chain_traits::types::ChainTypes;
use mercury_core::plugin::{
    self, AnyChain, ChainId, ChainPlugin, ChainStatusInfo, ClientStateInfo, DynRelayConfig,
};
use mercury_core::registry::ChainRegistry;
use mercury_relay::context::RelayWorkerConfig;
use mercury_relay::filter::PacketFilter;

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
    use mercury_core::plugin::{ChainPair, ClientMode};

    registry.register_chain(CosmosPlugin);
    registry.register_client_builder(
        ChainPair::new("cosmos", "cosmos", ClientMode::Default),
        Box::new(crate::client_builders::CosmosTendermintClientBuilder),
    );
}
