use async_trait::async_trait;
use ibc::core::client::types::Height;
use ibc::core::host::types::identifiers::ClientId;
use ibc_client_tendermint::types::{
    Header as TmIbcHeader,
    Misbehaviour as TmMisbehaviour,
};
use ibc_proto::google::protobuf::Any;
use ibc_proto::ibc::core::client::v1::{
    MsgUpdateClient, QueryConsensusStateHeightsRequest,
    query_client::QueryClient as IbcClientQueryClient,
};
use prost::Message as _;
use tendermint::block::Height as TmHeight;
use tendermint::validator::Set as ValidatorSet;
use tendermint_rpc::{Client, Paging};
use tracing::instrument;

use mercury_chain_traits::builders::{MisbehaviourDetector, MisbehaviourMessageBuilder};
use mercury_chain_traits::queries::MisbehaviourQuery;
use mercury_chain_traits::types::ChainTypes;
use mercury_core::error::Result;

use crate::builders::{CosmosUpdateClientPayload, fetch_trusted_state, find_proposer};
use crate::chain::CosmosChain;
use crate::client_types::CosmosClientState;
use crate::keys::CosmosSigner;
use crate::types::{CosmosMessage, to_any};
use mercury_core::MembershipProofs;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnChainTmConsensusState {
    pub height: TmHeight,
    pub timestamp_nanos: u128,
    pub root: [u8; 32],
    pub next_validators_hash: [u8; 32],
}

impl OnChainTmConsensusState {
    #[must_use]
    pub fn matches_fields(&self, other: &Self) -> bool {
        self.timestamp_nanos == other.timestamp_nanos
            && self.root == other.root
            && self.next_validators_hash == other.next_validators_hash
    }
}

pub async fn expected_consensus_state_at_height<S: CosmosSigner>(
    chain: &CosmosChain<S>,
    height: TmHeight,
) -> Result<OnChainTmConsensusState> {
    use tendermint_rpc::Client;

    let commit = chain
        .rpc_guard
        .guarded(|| async { chain.rpc_client.commit(height).await.map_err(Into::into) })
        .await?;

    let header = &commit.signed_header.header;

    let timestamp_nanos = header
        .time
        .unix_timestamp_nanos()
        .try_into()
        .map_err(|_| eyre::eyre!("timestamp out of u128 range"))?;

    let root: [u8; 32] = header
        .app_hash
        .as_bytes()
        .try_into()
        .map_err(|_| eyre::eyre!("app_hash is not 32 bytes"))?;

    let next_validators_hash: [u8; 32] = header
        .next_validators_hash
        .as_bytes()
        .try_into()
        .map_err(|_| eyre::eyre!("next_validators_hash is not 32 bytes"))?;

    Ok(OnChainTmConsensusState {
        height,
        timestamp_nanos,
        root,
        next_validators_hash,
    })
}

/// Fetches the correct header at `target_height` so it can be re-submitted to the
/// counterparty — the contract's `_checkUpdateResult` will see the conflict and freeze.
pub async fn build_corrective_update_payload<S: CosmosSigner>(
    chain: &CosmosChain<S>,
    trusted_height: TmHeight,
    target_height: TmHeight,
) -> Result<CosmosUpdateClientPayload> {
    let (trusted_consensus_state, trusted_next_validator_set) =
        fetch_trusted_state(chain, trusted_height).await?;

    let ibc_trusted_height = Height::new(chain.chain_id.revision_number(), trusted_height.value())
        .map_err(|e| eyre::eyre!("{e}"))?;

    let (commit_response, validators_response) = chain
        .rpc_guard
        .guarded_pair(
            || async {
                chain
                    .rpc_client
                    .commit(target_height)
                    .await
                    .map_err(eyre::Report::from)
            },
            || async {
                chain
                    .rpc_client
                    .validators(target_height, Paging::All)
                    .await
                    .map_err(eyre::Report::from)
            },
        )
        .await?;

    let proposer = find_proposer(
        &validators_response.validators,
        &commit_response.signed_header.header.proposer_address,
    );
    let validator_set = ValidatorSet::new(validators_response.validators, proposer);

    let header = TmIbcHeader {
        signed_header: commit_response.signed_header,
        validator_set,
        trusted_height: ibc_trusted_height,
        trusted_next_validator_set,
    };

    Ok(CosmosUpdateClientPayload {
        headers: vec![header.into()],
        trusted_consensus_state,
        membership_proofs: MembershipProofs::new(),
    })
}

#[derive(Clone, Debug)]
pub enum CosmosMisbehaviourEvidence {
    /// Two conflicting signed headers at the same height.
    Misbehaviour {
        misbehaviour: TmMisbehaviour,
        supporting_headers: Vec<TmIbcHeader>,
    },
    /// Re-submit the correct header so the contract detects the conflict and freezes.
    CorrectiveUpdate {
        payload: crate::builders::CosmosUpdateClientPayload,
    },
}

// TODO: break and refactor
#[async_trait]
impl<S: CosmosSigner> MisbehaviourDetector<Self> for CosmosChain<S> {
    type UpdateHeader = TmIbcHeader;
    type MisbehaviourEvidence = CosmosMisbehaviourEvidence;
    type CounterpartyClientState = CosmosClientState;

    #[instrument(skip_all, name = "check_for_misbehaviour", fields(chain = %self.chain_label()))]
    async fn check_for_misbehaviour(
        &self,
        client_id: &Self::ClientId,
        update_header: &Self::UpdateHeader,
        client_state: &Self::CounterpartyClientState,
    ) -> Result<Option<Self::MisbehaviourEvidence>> {
        let CosmosClientState::Tendermint(tm_cs) = client_state else {
            eyre::bail!("misbehaviour detection not supported for WASM clients");
        };

        if tm_cs.is_frozen() {
            tracing::info!("client is frozen, skipping misbehaviour check");
            return Ok(None);
        }

        let trusted_height_value = update_header.trusted_height.revision_height();
        let trusted_next_height = trusted_height_value
            .checked_add(1)
            .ok_or_else(|| eyre::eyre!("trusted_height + 1 overflows"))
            .and_then(|h| {
                TmHeight::try_from(h).map_err(|e| eyre::eyre!("invalid trusted_height + 1: {e}"))
            })?;

        let on_chain_trusted_validators = self
            .rpc_guard
            .guarded(|| async {
                self.rpc_client
                    .validators(trusted_next_height, Paging::All)
                    .await
                    .map_err(Into::into)
            })
            .await?;

        let on_chain_trusted_vs = ValidatorSet::new(on_chain_trusted_validators.validators, None);

        if on_chain_trusted_vs.hash() != update_header.trusted_next_validator_set.hash() {
            tracing::warn!(
                trusted_height = %update_header.trusted_height,
                "trusted validator set in update header does not match source chain, skipping"
            );
            return Ok(None);
        }

        let header_height = update_header.signed_header.header.height;

        let (commit_response, validators_response) = self
            .rpc_guard
            .guarded_pair(
                || async {
                    self.rpc_client
                        .commit(header_height)
                        .await
                        .map_err(eyre::Report::from)
                },
                || async {
                    self.rpc_client
                        .validators(header_height, Paging::All)
                        .await
                        .map_err(eyre::Report::from)
                },
            )
            .await?;

        let on_chain_header_hash = commit_response.signed_header.header.hash();
        let submitted_header_hash = update_header.signed_header.header.hash();

        if on_chain_header_hash == submitted_header_hash {
            return Ok(None);
        }

        tracing::error!(
            height = %header_height,
            submitted = %submitted_header_hash,
            on_chain = %on_chain_header_hash,
            "MISBEHAVIOUR DETECTED: conflicting headers at same height"
        );

        let proposer = validators_response
            .validators
            .iter()
            .find(|v| v.address == commit_response.signed_header.header.proposer_address)
            .cloned();
        let validator_set = ValidatorSet::new(validators_response.validators, proposer);

        let challenging_header = TmIbcHeader {
            signed_header: commit_response.signed_header,
            validator_set,
            trusted_height: update_header.trusted_height,
            trusted_next_validator_set: update_header.trusted_next_validator_set.clone(),
        };

        // header1 must have height >= header2 per TmMisbehaviour validation
        let misbehaviour =
            TmMisbehaviour::new(client_id.clone(), update_header.clone(), challenging_header);

        Ok(Some(CosmosMisbehaviourEvidence::Misbehaviour {
            misbehaviour,
            supporting_headers: Vec::new(),
        }))
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourMessageBuilder<Self> for CosmosChain<S> {
    type MisbehaviourEvidence = CosmosMisbehaviourEvidence;

    #[instrument(skip_all, name = "build_misbehaviour_message", fields(chain = %self.chain_label()))]
    async fn build_misbehaviour_message(
        &self,
        client_id: &ClientId,
        evidence: CosmosMisbehaviourEvidence,
    ) -> Result<CosmosMessage> {
        let CosmosMisbehaviourEvidence::Misbehaviour { misbehaviour, .. } = evidence else {
            eyre::bail!("corrective update is not applicable for native Tendermint clients");
        };

        let signer = self.signer.account_address()?;

        let misbehaviour_any: Any = misbehaviour.into();

        let msg = MsgUpdateClient {
            client_id: client_id.to_string(),
            client_message: Some(misbehaviour_any),
            signer,
        };

        Ok(to_any(&msg))
    }
}

/// Maximum number of consensus state heights to fetch per query.
const CONSENSUS_HEIGHTS_LIMIT: u64 = 1000;

#[async_trait]
impl<S: CosmosSigner> MisbehaviourQuery<Self> for CosmosChain<S> {
    type CounterpartyUpdateHeader = TmIbcHeader;

    #[instrument(skip_all, name = "query_consensus_state_heights", fields(chain = %self.chain_label(), client_id = %client_id))]
    async fn query_consensus_state_heights(
        &self,
        client_id: &Self::ClientId,
    ) -> Result<Vec<TmHeight>> {
        let pagination = Some(ibc_proto::cosmos::base::query::v1beta1::PageRequest {
            limit: CONSENSUS_HEIGHTS_LIMIT,
            reverse: true,
            ..Default::default()
        });

        let request = tonic::Request::new(QueryConsensusStateHeightsRequest {
            client_id: client_id.to_string(),
            pagination,
        });

        let response = self
            .rpc_guard
            .guarded(|| async {
                IbcClientQueryClient::new(self.grpc_channel.clone())
                    .consensus_state_heights(request)
                    .await
                    .map(tonic::Response::into_inner)
                    .map_err(Into::into)
            })
            .await?;

        let mut heights: Vec<TmHeight> = response
            .consensus_state_heights
            .iter()
            .filter_map(|h| {
                TmHeight::try_from(h.revision_height)
                    .inspect_err(|e| tracing::warn!(height = h.revision_height, error = %e, "invalid consensus height"))
                    .ok()
            })
            .collect();

        heights.sort_unstable_by(|a, b| b.cmp(a));

        Ok(heights)
    }

    #[instrument(skip_all, name = "query_update_client_header", fields(chain = %self.chain_label(), client_id = %client_id, height = %consensus_height))]
    async fn query_update_client_header(
        &self,
        client_id: &ClientId,
        consensus_height: &TmHeight,
    ) -> Result<Option<TmIbcHeader>> {
        use tendermint_rpc::query::{EventType, Query};

        let height_str = format!(
            "{}-{}",
            self.chain_id.revision_number(),
            consensus_height.value()
        );

        let query = Query::from(EventType::Tx)
            .and_eq("update_client.client_id", client_id.as_str())
            .and_eq("update_client.consensus_heights", height_str.as_str());

        let response = self
            .rpc_guard
            .guarded(|| async {
                self.rpc_client
                    .tx_search(query, false, 1, 1, tendermint_rpc::Order::Descending)
                    .await
                    .map_err(Into::into)
            })
            .await?;

        let Some(tx) = response.txs.first() else {
            return Ok(None);
        };

        for event in &tx.tx_result.events {
            if event.kind != "update_client" {
                continue;
            }

            let header_hex = event.attributes.iter().find_map(|attr| {
                let key = attr.key_str().ok()?;
                if key == "header" {
                    attr.value_str().ok()
                } else {
                    None
                }
            });

            if let Some(hex_str) = header_hex {
                let bytes = hex::decode(hex_str)?;
                let any = Any::decode(bytes.as_slice())?;
                let header = TmIbcHeader::try_from(any)
                    .map_err(|e| eyre::eyre!("failed to decode TmIbcHeader: {e}"))?;
                return Ok(Some(header));
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(height: u64, ts: u128, root: u8, val_hash: u8) -> OnChainTmConsensusState {
        OnChainTmConsensusState {
            height: TmHeight::try_from(height).unwrap(),
            timestamp_nanos: ts,
            root: [root; 32],
            next_validators_hash: [val_hash; 32],
        }
    }

    #[test]
    fn matches_fields_ignores_height() {
        let a = make_state(1, 1000, 0xAA, 0xBB);
        let b = make_state(99, 1000, 0xAA, 0xBB);
        assert!(a.matches_fields(&b));
    }

    #[test]
    fn matches_fields_detects_timestamp_diff() {
        let a = make_state(1, 1000, 0xAA, 0xBB);
        let b = make_state(1, 2000, 0xAA, 0xBB);
        assert!(!a.matches_fields(&b));
    }

    #[test]
    fn matches_fields_detects_root_diff() {
        let a = make_state(1, 1000, 0xAA, 0xBB);
        let b = make_state(1, 1000, 0xCC, 0xBB);
        assert!(!a.matches_fields(&b));
    }

    #[test]
    fn matches_fields_detects_validator_hash_diff() {
        let a = make_state(1, 1000, 0xAA, 0xBB);
        let b = make_state(1, 1000, 0xAA, 0xCC);
        assert!(!a.matches_fields(&b));
    }

    #[test]
    fn matches_fields_identical() {
        let a = make_state(5, 500, 0x11, 0x22);
        assert!(a.matches_fields(&a));
    }

    mod detector {
        use super::*;
        use crate::chain::test_support::{test_chain, test_signer, tm_client_state, tm_ibc_header};
        use crate::keys::Secp256k1KeyPair;
        use ibc_proto::ibc::lightclients::wasm::v1::ClientState as WasmClientState;
        use prost::Name;

        type TestChain = CosmosChain<Secp256k1KeyPair>;

        fn client_id() -> ClientId {
            "07-tendermint-0".parse().unwrap()
        }

        async fn check(
            chain: &TestChain,
            client_state: &CosmosClientState,
        ) -> Result<Option<CosmosMisbehaviourEvidence>> {
            <TestChain as MisbehaviourDetector<TestChain>>::check_for_misbehaviour(
                chain,
                &client_id(),
                &tm_ibc_header(10, 0xaa),
                client_state,
            )
            .await
        }

        #[tokio::test]
        async fn wasm_client_state_is_unsupported() {
            let chain = test_chain();
            let wasm = CosmosClientState::Wasm(WasmClientState::default());
            let err = check(&chain, &wasm).await.unwrap_err();
            assert!(err.to_string().contains("WASM"), "{err}");
        }

        #[tokio::test]
        async fn frozen_client_skips_check_without_rpc() {
            let chain = test_chain();
            let frozen = tm_client_state(5).with_frozen_height(Height::new(1, 5).unwrap());
            let result = check(&chain, &CosmosClientState::Tendermint(frozen))
                .await
                .unwrap();
            assert!(result.is_none());
        }

        #[tokio::test]
        async fn rpc_failure_is_an_error_not_a_clean_result() {
            let chain = test_chain();
            let active = CosmosClientState::Tendermint(tm_client_state(5));
            assert!(check(&chain, &active).await.is_err());
        }

        #[tokio::test]
        async fn max_trusted_height_is_an_error_not_an_overflow() {
            let chain = test_chain();
            let mut header = tm_ibc_header(10, 0xaa);
            header.trusted_height = Height::new(0, u64::MAX).unwrap();
            let active = CosmosClientState::Tendermint(tm_client_state(5));
            let err = <TestChain as MisbehaviourDetector<TestChain>>::check_for_misbehaviour(
                &chain,
                &client_id(),
                &header,
                &active,
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains("trusted_height + 1"), "{err}");
        }

        #[tokio::test]
        async fn misbehaviour_message_wraps_both_headers() {
            let chain = test_chain();
            let misbehaviour = TmMisbehaviour::new(
                client_id(),
                tm_ibc_header(10, 0xaa),
                tm_ibc_header(10, 0xbb),
            );
            let evidence = CosmosMisbehaviourEvidence::Misbehaviour {
                misbehaviour,
                supporting_headers: vec![],
            };
            let msg = chain
                .build_misbehaviour_message(&client_id(), evidence)
                .await
                .unwrap();

            assert_eq!(msg.type_url.as_ref(), MsgUpdateClient::type_url());
            let decoded = MsgUpdateClient::decode(msg.value.as_slice()).unwrap();
            assert_eq!(decoded.client_id, "07-tendermint-0");
            assert_eq!(decoded.signer, test_signer().account_address().unwrap());

            let any = decoded.client_message.unwrap();
            let round_trip = TmMisbehaviour::try_from(any).unwrap();
            assert_eq!(
                round_trip
                    .header1()
                    .signed_header
                    .header
                    .app_hash
                    .as_bytes(),
                &[0xaa; 32]
            );
            assert_eq!(
                round_trip
                    .header2()
                    .signed_header
                    .header
                    .app_hash
                    .as_bytes(),
                &[0xbb; 32]
            );
        }

        #[tokio::test]
        async fn corrective_update_rejected_for_native_tendermint() {
            let chain = test_chain();
            let evidence = CosmosMisbehaviourEvidence::CorrectiveUpdate {
                payload: crate::builders::CosmosUpdateClientPayload {
                    headers: vec![],
                    trusted_consensus_state: None,
                    membership_proofs: MembershipProofs::new(),
                },
            };
            assert!(
                chain
                    .build_misbehaviour_message(&client_id(), evidence)
                    .await
                    .is_err()
            );
        }
    }

    mod mocked {
        use super::*;
        use crate::chain::test_support::grpc::{GrpcReply, reply, spawn_grpc, unimplemented};
        use crate::chain::test_support::rpc::{MockRpc, Reply, RpcCall, spawn_rpc};
        use crate::chain::test_support::tm::{
            block_time, commit_reply, event, exec_result, header, set_hash, signed_header,
            tx_entry, tx_search_reply, validators, validators_reply,
        };
        use crate::chain::test_support::{UNREACHABLE_ADDR, mock_chain, tm_client_state};
        use crate::keys::Secp256k1KeyPair;
        use ibc_proto::ibc::core::client::v1::{
            Height as ProtoHeight, QueryConsensusStateHeightsResponse,
        };
        use tendermint::validator::Info;

        type TestChain = CosmosChain<Secp256k1KeyPair>;

        const HEIGHTS_PATH: &str = "/ibc.core.client.v1.Query/ConsensusStateHeights";

        fn client_id() -> ClientId {
            "07-tendermint-0".parse().unwrap()
        }

        fn height_param(call: &RpcCall) -> u64 {
            call.params["height"].as_str().unwrap().parse().unwrap()
        }

        // Node whose validator set at `height` is `vals_at(height)` and whose
        // canonical header at height h has app hash `app_hash_at(h)`.
        async fn node(
            vals_at: impl Fn(u64) -> Vec<Info> + Send + Sync + 'static,
            app_hash_at: impl Fn(u64) -> u8 + Send + Sync + 'static,
        ) -> MockRpc {
            spawn_rpc(move |call| {
                let h = height_param(call);
                match call.method.as_str() {
                    "validators" => validators_reply(h, &vals_at(h)),
                    "commit" => {
                        commit_reply(header(h, app_hash_at(h), &vals_at(h), &vals_at(h + 1)))
                    }
                    other => Reply::Error(format!("unexpected method {other}")),
                }
            })
            .await
        }

        fn heights_requested(rpc: &MockRpc, method: &str) -> Vec<u64> {
            let mut hs: Vec<u64> = rpc
                .calls()
                .iter()
                .filter(|c| c.method == method)
                .map(height_param)
                .collect();
            hs.sort_unstable();
            hs
        }

        // Update header at height 12 trusting height 9, as a relayer would submit it.
        fn update_header(app_hash: u8) -> TmIbcHeader {
            let vals = validators(&[1, 2, 3]);
            TmIbcHeader {
                signed_header: signed_header(header(12, app_hash, &vals, &vals)),
                validator_set: ValidatorSet::new(vals.clone(), Some(vals[0].clone())),
                trusted_height: Height::new(1, 9).unwrap(),
                trusted_next_validator_set: ValidatorSet::new(vals, None),
            }
        }

        async fn detect(
            rpc: &MockRpc,
            submitted: &TmIbcHeader,
        ) -> Result<Option<CosmosMisbehaviourEvidence>> {
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            <TestChain as MisbehaviourDetector<TestChain>>::check_for_misbehaviour(
                &chain,
                &client_id(),
                submitted,
                &CosmosClientState::Tendermint(tm_client_state(9)),
            )
            .await
        }

        #[tokio::test]
        async fn canonical_header_is_not_misbehaviour() {
            let rpc = node(|_| validators(&[1, 2, 3]), |_| 0xaa).await;
            assert!(detect(&rpc, &update_header(0xaa)).await.unwrap().is_none());
            // trusted validators come from trusted_height + 1, the header's own from 12
            assert_eq!(heights_requested(&rpc, "validators"), vec![10, 12]);
            assert_eq!(heights_requested(&rpc, "commit"), vec![12]);
        }

        #[tokio::test]
        async fn conflicting_header_yields_misbehaviour_with_on_chain_header() {
            let rpc = node(|_| validators(&[1, 2, 3]), |_| 0xbb).await;
            let submitted = update_header(0xaa);
            let evidence = detect(&rpc, &submitted).await.unwrap().unwrap();
            let CosmosMisbehaviourEvidence::Misbehaviour {
                misbehaviour,
                supporting_headers,
            } = evidence
            else {
                panic!("expected misbehaviour evidence");
            };
            assert_eq!(supporting_headers.len(), 0);
            assert_eq!(misbehaviour.client_id(), &client_id());
            assert_eq!(
                misbehaviour.header1().signed_header.header.hash(),
                submitted.signed_header.header.hash()
            );

            let on_chain = misbehaviour.header2();
            let vals = validators(&[1, 2, 3]);
            assert_eq!(
                on_chain.signed_header.header.hash(),
                header(12, 0xbb, &vals, &vals).hash()
            );
            // the validator set must hash to what the header commits to
            assert_eq!(
                on_chain.validator_set.hash(),
                on_chain.signed_header.header.validators_hash
            );
            assert_eq!(
                on_chain
                    .validator_set
                    .proposer()
                    .as_ref()
                    .map(|p| p.address),
                Some(vals[0].address)
            );
            assert_eq!(on_chain.trusted_height, submitted.trusted_height);
            assert_eq!(
                on_chain.trusted_next_validator_set.hash(),
                submitted.trusted_next_validator_set.hash()
            );
        }

        #[tokio::test]
        async fn foreign_trusted_validator_set_is_skipped_before_fetching_header() {
            let rpc = node(|_| validators(&[4, 5]), |_| 0xbb).await;
            assert!(detect(&rpc, &update_header(0xaa)).await.unwrap().is_none());
            assert_eq!(heights_requested(&rpc, "commit"), Vec::<u64>::new());
        }

        #[tokio::test]
        async fn commit_rpc_failure_is_an_error() {
            let rpc = spawn_rpc(|call| match call.method.as_str() {
                "validators" => validators_reply(height_param(call), &validators(&[1, 2, 3])),
                _ => Reply::Error(
                    "height 12 must be less than or equal to the current blockchain height 11"
                        .into(),
                ),
            })
            .await;
            assert!(detect(&rpc, &update_header(0xaa)).await.is_err());
        }

        #[tokio::test]
        async fn expected_consensus_state_mirrors_committed_header() {
            let rpc = node(
                |h| validators(if h > 30 { &[7, 8] } else { &[1] }),
                |_| 0x42,
            )
            .await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let state = expected_consensus_state_at_height(&chain, TmHeight::from(30u32))
                .await
                .unwrap();
            assert_eq!(state.height.value(), 30);
            assert_eq!(
                state.timestamp_nanos,
                u128::try_from(block_time(30).unix_timestamp_nanos()).unwrap()
            );
            assert_eq!(state.root, [0x42; 32]);
            // consensus state commits to the *next* validator set
            assert_eq!(
                state.next_validators_hash.as_slice(),
                set_hash(&validators(&[7, 8])).as_bytes()
            );
            assert_eq!(heights_requested(&rpc, "commit"), vec![30]);
        }

        #[tokio::test]
        async fn expected_consensus_state_rejects_non_32_byte_hashes() {
            let vals = validators(&[1]);
            let short_app_hash = {
                let mut h = header(5, 0, &vals, &vals);
                h.app_hash = tendermint::AppHash::try_from(vec![1u8; 20]).unwrap();
                h
            };
            let rpc = spawn_rpc(move |_| commit_reply(short_app_hash.clone())).await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let err = expected_consensus_state_at_height(&chain, TmHeight::from(5u32))
                .await
                .unwrap_err();
            assert!(err.to_string().contains("app_hash"), "{err:#}");

            let no_next_vals = {
                let mut h = header(5, 0, &vals, &vals);
                h.next_validators_hash = tendermint::Hash::None;
                h
            };
            let rpc = spawn_rpc(move |_| commit_reply(no_next_vals.clone())).await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let err = expected_consensus_state_at_height(&chain, TmHeight::from(5u32))
                .await
                .unwrap_err();
            assert!(err.to_string().contains("next_validators_hash"), "{err:#}");
        }

        #[tokio::test]
        async fn corrective_payload_carries_canonical_target_header() {
            let rpc = node(|_| validators(&[1, 2, 3]), |h| u8::try_from(h).unwrap()).await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let payload = build_corrective_update_payload(
                &chain,
                TmHeight::from(9u32),
                TmHeight::from(12u32),
            )
            .await
            .unwrap();

            assert_eq!(payload.headers.len(), 1);
            assert!(payload.membership_proofs.is_empty());
            let header = TmIbcHeader::try_from(payload.headers[0].clone()).unwrap();
            assert_eq!(header.signed_header.header.height.value(), 12);
            assert_eq!(header.signed_header.header.app_hash.as_bytes(), &[12u8; 32]);
            assert_eq!(
                header.validator_set.hash(),
                header.signed_header.header.validators_hash
            );
            assert_eq!(header.trusted_height, Height::new(1, 9).unwrap());

            let trusted = payload.trusted_consensus_state.unwrap();
            assert_eq!(trusted.root.as_bytes(), &[9u8; 32]);
            assert_eq!(trusted.timestamp(), block_time(9));
            // light clients require trusted validators to hash to the trusted
            // consensus state's next_validators_hash
            assert_eq!(
                header.trusted_next_validator_set.hash(),
                trusted.next_validators_hash
            );

            assert_eq!(heights_requested(&rpc, "validators"), vec![10, 12]);
            assert_eq!(heights_requested(&rpc, "commit"), vec![9, 12]);
        }

        #[tokio::test]
        async fn corrective_payload_rpc_failure_is_an_error() {
            let rpc = spawn_rpc(|_| Reply::Error("pruned".into())).await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            assert!(
                build_corrective_update_payload(
                    &chain,
                    TmHeight::from(9u32),
                    TmHeight::from(12u32)
                )
                .await
                .is_err()
            );
        }

        #[tokio::test]
        async fn consensus_state_heights_sorted_descending_and_invalid_dropped() {
            let grpc = spawn_grpc(|call| match call.path.as_str() {
                HEIGHTS_PATH => reply(&QueryConsensusStateHeightsResponse {
                    consensus_state_heights: [5, u64::MAX, 40, 12]
                        .into_iter()
                        .map(|h| ProtoHeight {
                            revision_number: 1,
                            revision_height: h,
                        })
                        .collect(),
                    pagination: None,
                }),
                _ => unimplemented(call),
            })
            .await;
            let chain = mock_chain(UNREACHABLE_ADDR, &grpc.url);
            let heights = chain
                .query_consensus_state_heights(&client_id())
                .await
                .unwrap();
            assert_eq!(
                heights.iter().map(TmHeight::value).collect::<Vec<_>>(),
                vec![40, 12, 5]
            );

            let req: QueryConsensusStateHeightsRequest = grpc.calls_to(HEIGHTS_PATH)[0].decode();
            assert_eq!(req.client_id, "07-tendermint-0");
            let page = req.pagination.unwrap();
            assert_eq!(page.limit, CONSENSUS_HEIGHTS_LIMIT);
            assert!(page.reverse, "newest heights first");
        }

        #[tokio::test]
        async fn consensus_state_heights_grpc_error_propagates() {
            let grpc = spawn_grpc(|_| GrpcReply::Err(tonic::Status::not_found("client"))).await;
            let chain = mock_chain(UNREACHABLE_ADDR, &grpc.url);
            assert!(
                chain
                    .query_consensus_state_heights(&client_id())
                    .await
                    .is_err()
            );
        }

        fn update_client_tx(attrs: &[(&str, &str)]) -> serde_json::Value {
            tx_entry(
                tendermint::Hash::Sha256([3; 32]),
                11,
                exec_result(
                    0,
                    "",
                    vec![
                        event(
                            "message",
                            &[("action", "/ibc.core.client.v1.MsgUpdateClient")],
                        ),
                        event("update_client", attrs),
                    ],
                ),
            )
        }

        async fn update_header_lookup(
            txs: Vec<serde_json::Value>,
        ) -> (Result<Option<TmIbcHeader>>, MockRpc) {
            let rpc = spawn_rpc(move |_| tx_search_reply(txs.clone())).await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let result = chain
                .query_update_client_header(&client_id(), &TmHeight::from(10u32))
                .await;
            (result, rpc)
        }

        #[tokio::test]
        async fn update_client_header_decoded_from_event() {
            let submitted = update_header(0xaa);
            let header_hex = hex::encode(Any::from(submitted.clone()).encode_to_vec());
            let (result, rpc) = update_header_lookup(vec![update_client_tx(&[
                ("client_id", "07-tendermint-0"),
                ("consensus_heights", "1-10"),
                ("header", &header_hex),
            ])])
            .await;
            let found = result.unwrap().unwrap();
            assert_eq!(
                found.signed_header.header.hash(),
                submitted.signed_header.header.hash()
            );
            assert_eq!(found.trusted_height, submitted.trusted_height);

            let params = &rpc.calls_to("tx_search")[0];
            assert_eq!(
                params["query"],
                "tm.event = 'Tx' AND update_client.client_id = '07-tendermint-0' AND update_client.consensus_heights = '1-10'"
            );
            assert_eq!(params["per_page"], "1");
            assert_eq!(params["order_by"], "desc");
        }

        #[tokio::test]
        async fn update_client_header_absent_is_none() {
            let (result, _) = update_header_lookup(vec![]).await;
            assert!(result.unwrap().is_none());

            // event exists but carries no header attribute (ibc-go dropped it)
            let (result, _) =
                update_header_lookup(vec![update_client_tx(&[("client_id", "07-tendermint-0")])])
                    .await;
            assert!(result.unwrap().is_none());
        }

        #[tokio::test]
        async fn update_client_header_malformed_is_error() {
            let (result, _) =
                update_header_lookup(vec![update_client_tx(&[("header", "zz")])]).await;
            assert!(result.is_err());

            let not_a_header = hex::encode(
                Any {
                    type_url: "/ibc.lightclients.wasm.v1.ClientMessage".into(),
                    value: vec![],
                }
                .encode_to_vec(),
            );
            let (result, _) =
                update_header_lookup(vec![update_client_tx(&[("header", &not_a_header)])]).await;
            assert!(result.is_err());
        }
    }
}
