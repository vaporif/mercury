use async_trait::async_trait;
use ibc::core::client::types::Height;
use ibc::core::host::types::identifiers::ClientId;
use ibc_client_tendermint::types::{
    ConsensusState as TendermintConsensusState, Header as TmIbcHeader,
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

use crate::builders::{CosmosUpdateClientPayload, find_proposer};
use mercury_core::MembershipProofs;
use crate::chain::CosmosChain;
use crate::client_types::CosmosClientState;
use crate::keys::CosmosSigner;
use crate::types::{CosmosMessage, to_any};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnChainTmConsensusState {
    pub height: TmHeight,
    pub timestamp_nanos: u128,
    pub root: [u8; 32],
    pub next_validators_hash: [u8; 32],
}

impl OnChainTmConsensusState {
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
    let (trusted_validators_response, trusted_commit_response) = chain
        .rpc_guard
        .guarded_pair(
            || async {
                chain
                    .rpc_client
                    .validators(trusted_height, Paging::All)
                    .await
                    .map_err(eyre::Report::from)
            },
            || async {
                chain
                    .rpc_client
                    .commit(trusted_height)
                    .await
                    .map_err(eyre::Report::from)
            },
        )
        .await?;

    let trusted_proposer = find_proposer(
        &trusted_validators_response.validators,
        &trusted_commit_response
            .signed_header
            .header
            .proposer_address,
    );
    let trusted_consensus_state = Some(TendermintConsensusState::from(
        trusted_commit_response.signed_header.header,
    ));
    let trusted_next_validator_set =
        ValidatorSet::new(trusted_validators_response.validators, trusted_proposer);

    let ibc_trusted_height =
        Height::new(chain.chain_id.revision_number(), trusted_height.value())
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
        let trusted_next_height = TmHeight::try_from(trusted_height_value + 1)
            .map_err(|e| eyre::eyre!("invalid trusted_height + 1: {e}"))?;

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
