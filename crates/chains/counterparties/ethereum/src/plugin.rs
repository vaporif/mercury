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
use crate::types::{EvmClientId, EvmHeight, EvmTxResponse};
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
