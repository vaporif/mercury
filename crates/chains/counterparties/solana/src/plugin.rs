use std::any::Any;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use mercury_chain_cache::CachedChain;
use mercury_chain_traits::builders::ClientPayloadBuilder;
use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use mercury_chain_traits::types::ChainTypes;
use mercury_core::plugin::{
    self, AnyChain, ChainId, ChainPlugin, ChainStatusInfo, ClientStateInfo,
};
use mercury_core::registry::ChainRegistry;

use mercury_solana::accounts::{OnChainClientState, deserialize_anchor_account};
use mercury_solana::config::SolanaChainConfig;
use mercury_solana::types::{SolanaClientId, SolanaHeight};

use crate::wrapper::SolanaAdapter;

type SolanaCached = CachedChain<SolanaAdapter>;

pub fn downcast_solana(chain: &AnyChain) -> eyre::Result<&SolanaCached> {
    (**chain)
        .downcast_ref::<SolanaCached>()
        .ok_or_else(|| eyre::eyre!("expected solana chain handle"))
}

async fn resolve_height(c: &SolanaCached, height: Option<u64>) -> eyre::Result<SolanaHeight> {
    if let Some(h) = height {
        Ok(SolanaHeight(h))
    } else {
        let status = c.query_chain_status().await?;
        Ok(*SolanaCached::chain_status_height(&status))
    }
}

fn table_to_solana_config(raw: &toml::Table) -> eyre::Result<SolanaChainConfig> {
    raw.clone()
        .try_into()
        .map_err(|e| eyre::eyre!("invalid solana config: {e}"))
}

struct SolanaPlugin;

#[async_trait]
impl ChainPlugin for SolanaPlugin {
    fn chain_type(&self) -> &'static str {
        "solana"
    }

    fn validate_config(&self, raw: &toml::Table) -> eyre::Result<()> {
        let cfg = table_to_solana_config(raw)?;
        cfg.validate()
    }

    async fn connect(
        &self,
        raw_config: &toml::Table,
        _config_dir: &Path,
    ) -> eyre::Result<AnyChain> {
        let cfg = table_to_solana_config(raw_config)?;
        let chain = SolanaAdapter::new_and_init(cfg).await?;
        Ok(Arc::new(CachedChain::new(chain)) as AnyChain)
    }

    fn parse_client_id(&self, raw: &str) -> eyre::Result<plugin::AnyClientId> {
        Ok(Box::new(SolanaClientId(raw.to_string())))
    }

    async fn query_status(&self, chain: &AnyChain) -> eyre::Result<ChainStatusInfo> {
        let c = downcast_solana(chain)?;
        let status = c.query_chain_status().await?;
        let height = SolanaCached::chain_status_height(&status);
        let timestamp = SolanaCached::chain_status_timestamp(&status);
        Ok(ChainStatusInfo {
            chain_id: ChainId::from(c.chain_id().to_string()),
            height: height.0,
            timestamp: timestamp.0.to_string(),
        })
    }

    fn chain_id_from_config(&self, _raw: &toml::Table) -> eyre::Result<ChainId> {
        Ok(ChainId::from("n/a"))
    }

    fn rpc_addr_from_config(&self, raw: &toml::Table) -> eyre::Result<String> {
        raw.get("rpc_addr")
            .and_then(toml::Value::as_str)
            .map(String::from)
            .ok_or_else(|| eyre::eyre!("missing 'rpc_addr' in solana config"))
    }

    async fn build_create_client_payload(
        &self,
        chain: &AnyChain,
    ) -> eyre::Result<Box<dyn Any + Send + Sync>> {
        let c = downcast_solana(chain)?;
        let payload =
            <SolanaCached as ClientPayloadBuilder<SolanaAdapter>>::build_create_client_payload(c)
                .await?;
        Ok(Box::new(payload))
    }

    async fn create_client(
        &self,
        chain: &AnyChain,
        payload: Box<dyn Any + Send + Sync>,
    ) -> eyre::Result<String> {
        use mercury_chain_traits::builders::ClientMessageBuilder;
        use mercury_chain_traits::types::MessageSender;
        use mercury_cosmos::builders::CosmosCreateClientPayload;
        use mercury_cosmos::keys::Secp256k1KeyPair;

        let c = downcast_solana(chain)?;
        if let Some(cosmos_payload) = payload.downcast_ref::<CosmosCreateClientPayload>() {
            let msg = ClientMessageBuilder::<
                mercury_cosmos::chain::CosmosChain<Secp256k1KeyPair>,
            >::build_create_client_message(c, cosmos_payload.clone())
            .await?;

            c.send_messages(vec![msg]).await?;
            return Ok(crate::DEFAULT_TENDERMINT_CLIENT_ID.to_string());
        }

        eyre::bail!("unsupported payload type for Solana create_client")
    }

    async fn query_client_state_info(
        &self,
        chain: &AnyChain,
        client_id: &str,
        height: Option<u64>,
    ) -> eyre::Result<ClientStateInfo> {
        let c = downcast_solana(chain)?;
        let cid = SolanaClientId(client_id.to_string());
        let h = resolve_height(c, height).await?;
        let cs = ClientQuery::<mercury_solana::chain::SolanaChain>::query_client_state(c, &cid, &h)
            .await?;
        let parsed: OnChainClientState = deserialize_anchor_account(&cs.0)?;
        Ok(ClientStateInfo {
            client_id: client_id.to_string(),
            latest_height: parsed.latest_height.revision_height,
            trusting_period: Some(std::time::Duration::from_secs(parsed.trusting_period)),
            frozen: parsed.frozen_height.revision_height > 0,
            client_type: "07-tendermint".to_string(),
            chain_id: parsed.chain_id,
        })
    }

    async fn query_commitment_sequences(
        &self,
        chain: &AnyChain,
        client_id: &str,
        height: Option<u64>,
    ) -> eyre::Result<Vec<u64>> {
        let c = downcast_solana(chain)?;
        let cid = SolanaClientId(client_id.to_string());
        let h = resolve_height(c, height).await?;
        let seqs = PacketStateQuery::query_commitment_sequences(c, &cid, &h).await?;
        Ok(seqs.into_iter().map(|s| s.0).collect())
    }

    async fn build_update_client_payload(
        &self,
        chain: &AnyChain,
        trusted_height: u64,
        target_height: u64,
        _counterparty_client_state: Option<&(dyn Any + Send + Sync)>,
    ) -> eyre::Result<Box<dyn Any + Send + Sync>> {
        let c = downcast_solana(chain)?;
        let payload =
            <SolanaCached as ClientPayloadBuilder<SolanaAdapter>>::build_update_client_payload(
                c,
                &SolanaHeight(trusted_height),
                &SolanaHeight(target_height),
                &mercury_solana::types::SolanaClientState(Vec::new()),
            )
            .await?;
        Ok(Box::new(payload))
    }

    async fn update_client(
        &self,
        chain: &AnyChain,
        client_id: &str,
        payload: Box<dyn Any + Send + Sync>,
    ) -> eyre::Result<()> {
        use mercury_chain_traits::builders::ClientMessageBuilder;
        use mercury_chain_traits::types::MessageSender;
        use mercury_cosmos::builders::CosmosUpdateClientPayload;
        use mercury_cosmos::keys::Secp256k1KeyPair;

        let c = downcast_solana(chain)?;
        let parsed_id = SolanaClientId(client_id.to_string());

        if let Some(cosmos_payload) = payload.downcast_ref::<CosmosUpdateClientPayload>() {
            let output = ClientMessageBuilder::<
                mercury_cosmos::chain::CosmosChain<Secp256k1KeyPair>,
            >::build_update_client_message(c, &parsed_id, cosmos_payload.clone())
            .await?;

            c.send_messages(output.messages).await?;
            return Ok(());
        }

        eyre::bail!("unsupported payload type for Solana update_client")
    }
}

pub fn register(registry: &mut ChainRegistry) {
    registry.register_chain(SolanaPlugin);
}
