use async_trait::async_trait;
use ibc::core::host::types::identifiers::ClientId;
use ibc_client_tendermint::types::{Header as TmIbcHeader, Misbehaviour as TmMisbehaviour};
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
use mercury_core::error::Result;

use crate::chain::CosmosChainInner;
use crate::client_types::CosmosClientState;
use crate::keys::CosmosSigner;
use crate::types::{CosmosMessage, to_any};

/// Evidence of light client misbehaviour on a Cosmos chain.
#[derive(Clone, Debug)]
pub struct CosmosMisbehaviourEvidence {
    pub misbehaviour: TmMisbehaviour,
    pub supporting_headers: Vec<TmIbcHeader>,
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourDetector<Self> for CosmosChainInner<S> {
    type UpdateHeader = TmIbcHeader;
    type MisbehaviourEvidence = CosmosMisbehaviourEvidence;
    type CounterpartyClientState = CosmosClientState;

    #[instrument(skip_all, name = "check_for_misbehaviour")]
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

        // Validate trusted validator set against source chain to prevent fabricated headers
        let trusted_height_value = update_header.trusted_height.revision_height();
        let trusted_next_height = TmHeight::try_from(trusted_height_value + 1)
            .map_err(|e| eyre::eyre!("invalid trusted_height + 1: {e}"))?;

        let on_chain_trusted_validators = self
            .rpc_client
            .validators(trusted_next_height, Paging::All)
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

        let (commit_response, validators_response) = tokio::try_join!(
            async {
                self.rpc_client
                    .commit(header_height)
                    .await
                    .map_err(eyre::Report::from)
            },
            async {
                self.rpc_client
                    .validators(header_height, Paging::All)
                    .await
                    .map_err(eyre::Report::from)
            },
        )?;

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

        Ok(Some(CosmosMisbehaviourEvidence {
            misbehaviour,
            supporting_headers: Vec::new(),
        }))
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourMessageBuilder<Self> for CosmosChainInner<S> {
    type MisbehaviourEvidence = CosmosMisbehaviourEvidence;

    #[instrument(skip_all, name = "build_misbehaviour_message")]
    async fn build_misbehaviour_message(
        &self,
        client_id: &ClientId,
        evidence: CosmosMisbehaviourEvidence,
    ) -> Result<CosmosMessage> {
        let signer = self.signer.account_address()?;

        let misbehaviour_any: Any = evidence.misbehaviour.into();

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
impl<S: CosmosSigner> MisbehaviourQuery<Self> for CosmosChainInner<S> {
    type CounterpartyUpdateHeader = TmIbcHeader;

    #[instrument(skip_all, name = "query_consensus_state_heights", fields(client_id = %client_id))]
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

        let response = IbcClientQueryClient::new(self.grpc_channel.clone())
            .consensus_state_heights(request)
            .await?
            .into_inner();

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

    #[instrument(skip_all, name = "query_update_client_header", fields(client_id = %client_id, height = %consensus_height))]
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
            .rpc_client
            .tx_search(query, false, 1, 1, tendermint_rpc::Order::Descending)
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
