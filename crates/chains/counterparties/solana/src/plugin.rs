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

#[cfg(test)]
mod tests {
    use mercury_core::plugin::ChainPlugin;
    use mercury_solana::accounts::{Ics07Tendermint, Ics26Router};
    use mercury_solana::chain::testing::test_chain;
    use mercury_solana::rpc::testing::{FakeRpc, anchor_account};
    use solana_sdk::pubkey::Pubkey;
    use solana_sdk::signer::Signer;
    use solana_sdk::signer::keypair::Keypair;

    use super::*;

    const CLIENT: &str = "07-tendermint-0";

    fn any_chain(rpc: FakeRpc, ics26: Pubkey, ics07: Option<Pubkey>) -> AnyChain {
        let mut chain = test_chain(ics26, rpc);
        chain.ics07_program_id = ics07;
        Arc::new(CachedChain::new(SolanaAdapter(chain))) as AnyChain
    }

    fn write_keypair() -> (std::path::PathBuf, Keypair) {
        let kp = Keypair::new();
        let path = std::env::temp_dir().join(format!("mercury-solana-plugin-{}.json", kp.pubkey()));
        std::fs::write(&path, serde_json_bytes(&kp.to_bytes())).unwrap();
        (path, kp)
    }

    fn serde_json_bytes(bytes: &[u8]) -> String {
        let items: Vec<String> = bytes.iter().map(u8::to_string).collect();
        format!("[{}]", items.join(","))
    }

    fn table(src: &str) -> toml::Table {
        toml::from_str(src).unwrap()
    }

    fn valid_table(keypair: &Path, program_id: &Pubkey) -> toml::Table {
        table(&format!(
            r#"
            rpc_addr = "http://127.0.0.1:8899"
            program_id = "{program_id}"
            keypair_path = "{}"
            "#,
            keypair.display()
        ))
    }

    // On-chain ics07 `ClientState` layout (see programs/ics07-tendermint/src/types.rs).
    type WireClientState = (String, u64, u64, u64, u64, u64, (u64, u64), (u64, u64));

    fn seeded_client(ics26: &Pubkey, ics07: &Pubkey, frozen_at: u64) -> FakeRpc {
        let cs: WireClientState = (
            "cosmoshub-4".into(),
            1,
            3,
            86_400,
            172_800,
            10,
            (0, frozen_at),
            (4, 321),
        );
        FakeRpc::default()
            .with_account(
                Ics26Router::client_pda(CLIENT, ics26).0,
                anchor_account("Client", &(1u8, CLIENT.to_string(), ics07.to_bytes())),
            )
            .with_account(
                Ics07Tendermint::client_state_pda(ics07).0,
                anchor_account("ClientState", &cs),
            )
    }

    #[test]
    fn config_helpers_parse_and_validate() {
        let (path, _) = write_keypair();
        let program = Pubkey::new_unique();
        let raw = valid_table(&path, &program);

        assert_eq!(SolanaPlugin.chain_type(), "solana");
        SolanaPlugin.validate_config(&raw).unwrap();
        assert_eq!(
            SolanaPlugin.rpc_addr_from_config(&raw).unwrap(),
            "http://127.0.0.1:8899"
        );
        assert_eq!(
            SolanaPlugin.chain_id_from_config(&raw).unwrap(),
            ChainId::from("n/a")
        );

        let mut no_rpc = raw.clone();
        no_rpc.remove("rpc_addr");
        let err = SolanaPlugin.rpc_addr_from_config(&no_rpc).unwrap_err();
        assert!(err.to_string().contains("missing 'rpc_addr'"), "{err}");
        let err = SolanaPlugin.validate_config(&no_rpc).unwrap_err();
        assert!(err.to_string().contains("invalid solana config"), "{err}");

        let mut bad_url = raw;
        bad_url.insert("rpc_addr".into(), "not a url".into());
        assert!(SolanaPlugin.validate_config(&bad_url).is_err());

        let missing_key = valid_table(Path::new("/definitely/missing.json"), &program);
        assert!(SolanaPlugin.validate_config(&missing_key).is_err());

        let id = SolanaPlugin.parse_client_id(CLIENT).unwrap();
        assert_eq!(id.downcast_ref::<SolanaClientId>().unwrap().0, CLIENT);

        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn connect_builds_a_solana_handle_from_config() {
        let (path, kp) = write_keypair();
        let program = Pubkey::new_unique();
        let chain = SolanaPlugin
            .connect(&valid_table(&path, &program), Path::new("."))
            .await
            .unwrap();
        let c = downcast_solana(&chain).unwrap();
        assert_eq!(c.inner().0.ics26_program_id, program);
        assert_eq!(c.inner().0.keypair.pubkey(), kp.pubkey());

        let mut bad = valid_table(&path, &program);
        bad.insert("program_id".into(), "not-a-pubkey".into());
        assert!(SolanaPlugin.connect(&bad, Path::new(".")).await.is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn downcast_rejects_foreign_handles() {
        let foreign: AnyChain = Arc::new(42u8);
        let Err(err) = downcast_solana(&foreign) else {
            panic!("a u8 is not a solana handle");
        };
        assert!(err.to_string().contains("expected solana chain handle"));
        assert!(SolanaPlugin.query_status(&foreign).await.is_err());
    }

    #[tokio::test]
    async fn query_status_reports_slot_and_block_time() {
        let rpc = FakeRpc {
            slot: Some(900),
            block_time: Some(1_700_000_000),
            ..FakeRpc::default()
        };
        let chain = any_chain(rpc, Pubkey::new_unique(), None);
        let status = SolanaPlugin.query_status(&chain).await.unwrap();
        assert_eq!(status.height, 900);
        assert_eq!(status.timestamp, "1700000000");
        // Solana has no chain id; status shows the placeholder (config reports "n/a").
        assert_eq!(status.chain_id, ChainId::from("<n/a>"));
    }

    #[tokio::test]
    async fn client_state_info_decodes_ics07_client_state() {
        let (ics26, ics07) = (Pubkey::new_unique(), Pubkey::new_unique());
        let chain = any_chain(seeded_client(&ics26, &ics07, 0), ics26, Some(ics07));

        let info = SolanaPlugin
            .query_client_state_info(&chain, CLIENT, Some(5))
            .await
            .unwrap();
        assert_eq!(info.client_id, CLIENT);
        assert_eq!(info.latest_height, 321);
        assert_eq!(
            info.trusting_period,
            Some(std::time::Duration::from_hours(24))
        );
        assert!(!info.frozen);
        assert_eq!(info.client_type, "07-tendermint");
        assert_eq!(info.chain_id, "cosmoshub-4");

        let (ics26, ics07) = (Pubkey::new_unique(), Pubkey::new_unique());
        let mut rpc = seeded_client(&ics26, &ics07, 7);
        rpc.slot = Some(10);
        rpc.block_time = Some(1);
        let frozen = any_chain(rpc, ics26, Some(ics07));
        // No height given: resolves the latest slot first.
        let info = SolanaPlugin
            .query_client_state_info(&frozen, CLIENT, None)
            .await
            .unwrap();
        assert!(info.frozen);

        assert!(
            SolanaPlugin
                .query_client_state_info(&chain, "07-tendermint-9", Some(1))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn commitment_sequences_resolve_height_then_scan() {
        let ics26 = Pubkey::new_unique();
        let rpc = FakeRpc {
            slot: Some(10),
            block_time: Some(1),
            ..FakeRpc::default()
        }
        .with_account(
            Ics26Router::packet_commitment_pda(CLIENT, 1, &ics26).0,
            anchor_account("Commitment", &[0xC0u8; 32]),
        )
        .with_account(
            Ics26Router::packet_commitment_pda(CLIENT, 3, &ics26).0,
            anchor_account("Commitment", &[0xC1u8; 32]),
        );
        let chain = any_chain(rpc, ics26, None);
        assert_eq!(
            SolanaPlugin
                .query_commitment_sequences(&chain, CLIENT, None)
                .await
                .unwrap(),
            vec![1, 3]
        );

        let no_slot = any_chain(FakeRpc::default(), ics26, None);
        assert!(
            SolanaPlugin
                .query_commitment_sequences(&no_slot, CLIENT, None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn client_payloads_are_built_from_the_latest_slot() {
        let rpc = FakeRpc {
            slot: Some(77),
            block_time: Some(1_234),
            ..FakeRpc::default()
        };
        let chain = any_chain(rpc, Pubkey::new_unique(), None);

        let create = SolanaPlugin
            .build_create_client_payload(&chain)
            .await
            .unwrap();
        let create = create
            .downcast_ref::<mercury_solana::types::SolanaCreateClientPayload>()
            .unwrap();
        assert!(matches!(
            create,
            mercury_solana::types::SolanaCreateClientPayload::Dummy { client_state, .. }
                if *client_state == 77u64.to_le_bytes()
        ));

        let update = SolanaPlugin
            .build_update_client_payload(&chain, 1, 77, None)
            .await
            .unwrap();
        assert!(
            update
                .downcast_ref::<mercury_solana::types::SolanaUpdateClientPayload>()
                .is_some()
        );
    }

    #[tokio::test]
    async fn create_and_update_reject_foreign_payloads() {
        let chain = any_chain(FakeRpc::default(), Pubkey::new_unique(), None);
        let err = SolanaPlugin
            .create_client(&chain, Box::new(1u8))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("unsupported payload type"),
            "{err}"
        );
        let err = SolanaPlugin
            .update_client(&chain, CLIENT, Box::new(1u8))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("unsupported payload type"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn create_client_sends_add_client_and_initialize() {
        use ibc::core::client::types::Height;
        use ibc::core::commitment_types::commitment::CommitmentRoot;
        use ibc::core::commitment_types::specs::ProofSpecs;
        use ibc::core::host::types::identifiers::ChainId as IbcChainId;
        use ibc_client_tendermint::types::{AllowUpdate, ConsensusState, TrustThreshold};
        use mercury_cosmos::builders::CosmosCreateClientPayload;

        let (ics26, ics07, am) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let rpc = FakeRpc::default().with_account(
            Ics26Router::router_state_pda(&ics26).0,
            anchor_account(
                "RouterState",
                &(1u8, (am.to_bytes(), None::<[u8; 32]>), false),
            ),
        );
        let sent = rpc.sent_txs.clone();
        let chain = any_chain(rpc, ics26, Some(ics07));

        let cs = ibc_client_tendermint::types::ClientState::new(
            IbcChainId::new("cosmoshub-4").unwrap(),
            TrustThreshold::ONE_THIRD,
            std::time::Duration::from_secs(100),
            std::time::Duration::from_secs(200),
            std::time::Duration::from_secs(10),
            Height::new(4, 50).unwrap(),
            ProofSpecs::cosmos(),
            vec![],
            AllowUpdate {
                after_expiry: false,
                after_misbehaviour: false,
            },
        )
        .unwrap();
        let cons = ConsensusState::new(
            CommitmentRoot::from_bytes(&[1; 32]),
            tendermint::Time::from_unix_timestamp(1_700_000_000, 0).unwrap(),
            tendermint::Hash::Sha256([2; 32]),
        );
        let payload = CosmosCreateClientPayload {
            client_state: cs.into(),
            consensus_state: cons.into(),
            counterparty_client_id: Some("08-wasm-0".into()),
            counterparty_merkle_prefix: None,
        };

        let id = SolanaPlugin
            .create_client(&chain, Box::new(payload))
            .await
            .unwrap();
        assert_eq!(id, crate::DEFAULT_TENDERMINT_CLIENT_ID);
        // add_client and initialize each carry their own compute budget, so two txs.
        assert_eq!(sent.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn update_client_sends_header_upload_assemble_and_cleanup() {
        use crate::test_support::{deployed, tm_header};
        use mercury_cosmos::builders::CosmosUpdateClientPayload;

        let (d, rpc) = deployed(CLIENT);
        let sent = rpc.sent_txs.clone();
        let chain = any_chain(rpc, d.ics26, Some(d.ics07));
        let payload = |headers: Vec<ibc_client_tendermint::types::Header>| {
            Box::new(CosmosUpdateClientPayload {
                headers: headers.into_iter().map(Into::into).collect(),
                trusted_consensus_state: None,
                membership_proofs: mercury_core::MembershipProofs::default(),
            })
        };

        SolanaPlugin
            .update_client(
                &chain,
                CLIENT,
                payload(vec![tm_header(&[10], &[0], 20, 10)]),
            )
            .await
            .unwrap();
        // A one-validator header fits one chunk: upload, assemble, cleanup.
        assert_eq!(sent.lock().unwrap().len(), 3);

        let err = SolanaPlugin
            .update_client(&chain, CLIENT, payload(vec![]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no headers"), "{err}");
    }

    #[test]
    fn register_adds_the_solana_plugin() {
        let mut registry = ChainRegistry::new();
        register(&mut registry);
        assert_eq!(registry.chain("solana").unwrap().chain_type(), "solana");
    }
}
