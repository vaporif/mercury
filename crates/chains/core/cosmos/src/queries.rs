use async_trait::async_trait;
use ibc_client_tendermint::types::ClientState as TendermintClientState;
use ibc_client_tendermint::types::ConsensusState as TendermintConsensusState;
use ibc_proto::Protobuf;
use ibc_proto::ibc::core::client::v1::QueryClientStateRequest;
use ibc_proto::ibc::core::client::v1::QueryConsensusStateRequest;
use ibc_proto::ibc::core::client::v1::query_client::QueryClient as IbcClientQueryClient;
use prost::Message;
use tendermint::block::Height as TmHeight;
use tendermint_rpc::{Client, HttpClient};
use tracing::{instrument, warn};

use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use mercury_chain_traits::types::{ChainTypes, PacketSequence};
use mercury_core::error::{ProofError, QueryError, Result};

use crate::chain::CosmosChain;
use crate::client_types::{
    CosmosClientState, CosmosConsensusState, TENDERMINT_CLIENT_STATE_TYPE_URL,
    TENDERMINT_CONSENSUS_STATE_TYPE_URL, WASM_CLIENT_STATE_TYPE_URL, WASM_CONSENSUS_STATE_TYPE_URL,
};
use crate::keys::CosmosSigner;
use crate::types::{
    CosmosChainStatus, MerkleProof, PacketAcknowledgement, PacketCommitment, PacketReceipt,
};

pub(crate) async fn query_abci(
    client: &HttpClient,
    rpc_guard: &mercury_core::rpc_guard::RpcGuard,
    path: &str,
    data: Vec<u8>,
    height: Option<TmHeight>,
    prove: bool,
) -> Result<tendermint_rpc::endpoint::abci_query::AbciQuery> {
    rpc_guard
        .guarded(|| async {
            client
                .abci_query(Some(path.to_string()), data, height, prove)
                .await
                .map_err(Into::into)
        })
        .await
}

pub async fn query_cosmos_status(rpc_addr: &str) -> Result<CosmosChainStatus> {
    let client = HttpClient::new(rpc_addr)?;
    let status = client.status().await?;
    Ok(CosmosChainStatus {
        height: status.sync_info.latest_block_height,
        timestamp: status.sync_info.latest_block_time,
    })
}

#[async_trait]
impl<S: CosmosSigner> ChainStatusQuery for CosmosChain<S> {
    #[instrument(skip_all, name = "query_chain_status", fields(chain = %self.chain_label()))]
    async fn query_chain_status(&self) -> Result<Self::ChainStatus> {
        let status = self
            .rpc_guard
            .guarded(|| async { self.rpc_client.status().await.map_err(Into::into) })
            .await?;
        Ok(CosmosChainStatus {
            height: status.sync_info.latest_block_height,
            timestamp: status.sync_info.latest_block_time,
        })
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientQuery<Self> for CosmosChain<S> {
    #[instrument(skip_all, name = "query_client_state", fields(chain = %self.chain_label(), client_id = %client_id))]
    async fn query_client_state(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Self::ClientState> {
        let mut request = tonic::Request::new(QueryClientStateRequest {
            client_id: client_id.to_string(),
        });

        request
            .metadata_mut()
            .insert("x-cosmos-block-height", height.value().to_string().parse()?);

        let response = self
            .rpc_guard
            .guarded(|| async {
                IbcClientQueryClient::new(self.grpc_channel.clone())
                    .client_state(request)
                    .await
                    .map(tonic::Response::into_inner)
                    .map_err(Into::into)
            })
            .await?;

        let any = response
            .client_state
            .ok_or_else(|| QueryError::StaleState {
                what: format!("client state for {client_id}"),
            })?;

        let type_url = any.type_url.strip_prefix('/').unwrap_or(&any.type_url);

        let client_state = match type_url {
            TENDERMINT_CLIENT_STATE_TYPE_URL => {
                let cs = <TendermintClientState as Protobuf<
                    ibc_client_tendermint::types::proto::v1::ClientState,
                >>::decode(any.value.as_slice())?;
                CosmosClientState::Tendermint(cs)
            }
            WASM_CLIENT_STATE_TYPE_URL => {
                let cs = ibc_proto::ibc::lightclients::wasm::v1::ClientState::decode(
                    any.value.as_slice(),
                )?;
                CosmosClientState::Wasm(cs)
            }
            other => {
                return Err(QueryError::UnsupportedType {
                    type_url: other.to_string(),
                }
                .into());
            }
        };
        Ok(client_state)
    }

    #[instrument(skip_all, name = "query_consensus_state", fields(chain = %self.chain_label(), client_id = %client_id))]
    async fn query_consensus_state(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &<Self as ChainTypes>::Height,
        query_height: &Self::Height,
    ) -> Result<Self::ConsensusState> {
        let revision_height = consensus_height.value();
        let revision_number = self.chain_id.revision_number();

        let mut request = tonic::Request::new(QueryConsensusStateRequest {
            client_id: client_id.to_string(),
            revision_number,
            revision_height,
            latest_height: false,
        });

        request.metadata_mut().insert(
            "x-cosmos-block-height",
            query_height.value().to_string().parse()?,
        );

        let response = self
            .rpc_guard
            .guarded(|| async {
                IbcClientQueryClient::new(self.grpc_channel.clone())
                    .consensus_state(request)
                    .await
                    .map(tonic::Response::into_inner)
                    .map_err(Into::into)
            })
            .await?;

        let any = response
            .consensus_state
            .ok_or_else(|| QueryError::StaleState {
                what: format!("consensus state for {client_id} at height {consensus_height}"),
            })?;

        let type_url = any.type_url.strip_prefix('/').unwrap_or(&any.type_url);

        let consensus_state = match type_url {
            TENDERMINT_CONSENSUS_STATE_TYPE_URL => {
                let cs = <TendermintConsensusState as Protobuf<
                    ibc_client_tendermint::types::proto::v1::ConsensusState,
                >>::decode(any.value.as_slice())?;
                CosmosConsensusState::Tendermint(cs)
            }
            WASM_CONSENSUS_STATE_TYPE_URL => {
                let cs = ibc_proto::ibc::lightclients::wasm::v1::ConsensusState::decode(
                    any.value.as_slice(),
                )?;
                CosmosConsensusState::Wasm(cs)
            }
            other => {
                return Err(QueryError::UnsupportedType {
                    type_url: other.to_string(),
                }
                .into());
            }
        };
        Ok(consensus_state)
    }

    fn trusting_period(client_state: &Self::ClientState) -> Option<std::time::Duration> {
        match client_state {
            CosmosClientState::Tendermint(cs) => Some(cs.trusting_period),
            CosmosClientState::Wasm(_) => None,
        }
    }

    fn client_latest_height(client_state: &Self::ClientState) -> Self::Height {
        match client_state {
            CosmosClientState::Tendermint(cs) => {
                let h = cs.latest_height.revision_height();
                TmHeight::try_from(h.max(1)).unwrap_or_else(|_| TmHeight::from(1_u32))
            }
            CosmosClientState::Wasm(cs) => cs
                .latest_height
                .as_ref()
                .and_then(|h| TmHeight::try_from(h.revision_height.max(1)).ok())
                .unwrap_or_else(|| {
                    tracing::warn!("WASM client state missing latest_height, defaulting to 1");
                    TmHeight::from(1_u32)
                }),
        }
    }
}

const IBC_STORE_PATH: &str = "store/ibc/key";

const COMMITMENT_DISCRIMINATOR: u8 = 0x01;
const RECEIPT_DISCRIMINATOR: u8 = 0x02;
const ACK_DISCRIMINATOR: u8 = 0x03;

/// ABCI commits state at H in block H+1's `app_hash`, so proofs
/// must be queried at H-1 to match the client's known root.
fn proof_query_height(height: TmHeight) -> Result<TmHeight> {
    let prev = height
        .value()
        .checked_sub(1)
        .ok_or_else(|| eyre::eyre!("proof height underflow: height is 0"))?;
    let h = TmHeight::try_from(prev)
        .map_err(|e| eyre::eyre!("invalid proof query height {prev}: {e}"))?;
    Ok(h)
}

/// IBC v2 key: `client_bytes` || `discriminator` || `sequence_be_bytes`
fn ibc_v2_key(client: &str, discriminator: u8, sequence: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(client.len() + 1 + 8);
    key.extend_from_slice(client.as_bytes());
    key.push(discriminator);
    key.extend_from_slice(&sequence.to_be_bytes());
    key
}

pub(crate) fn extract_proof(
    response: &tendermint_rpc::endpoint::abci_query::AbciQuery,
) -> Result<MerkleProof> {
    let proof_ops = response.proof.as_ref().ok_or(ProofError::Missing)?;

    let proofs: Vec<ibc_proto::ics23::CommitmentProof> = proof_ops
        .ops
        .iter()
        .filter_map(
            |op| match ibc_proto::ics23::CommitmentProof::decode(op.data.as_slice()) {
                Ok(proof) => Some(proof),
                Err(e) => {
                    warn!("failed to decode CommitmentProof op: {e}");
                    None
                }
            },
        )
        .collect();

    let merkle_proof = ibc_proto::ibc::core::commitment::v1::MerkleProof { proofs };
    Ok(MerkleProof {
        proof_bytes: merkle_proof.encode_to_vec(),
    })
}

async fn paginated_packet_sequences<S: CosmosSigner>(
    chain: &CosmosChain<S>,
    event_kind: &str,
    client_id: &ibc::core::host::types::identifiers::ClientId,
    client_field: fn(&ibc_proto::ibc::core::channel::v2::Packet) -> &str,
) -> Result<Vec<PacketSequence>> {
    use std::collections::HashSet;
    use tendermint_rpc::query::{EventType, Query};

    let query = Query::from(EventType::Tx).and_exists(format!("{event_kind}.encoded_packet_hex"));

    let mut sequences = HashSet::new();
    let mut page = 1u32;
    let per_page = 100u8;

    loop {
        let response = chain
            .rpc_guard
            .guarded(|| async {
                chain
                    .rpc_client
                    .tx_search(
                        query.clone(),
                        false,
                        page,
                        per_page,
                        tendermint_rpc::Order::Ascending,
                    )
                    .await
                    .map_err(Into::into)
            })
            .await?;

        for tx in &response.txs {
            for event in &tx.tx_result.events {
                if event.kind != event_kind {
                    continue;
                }
                let hex_attr = event.attributes.iter().find_map(|attr| {
                    let key = attr.key_str().ok()?;
                    if key == "encoded_packet_hex" {
                        attr.value_str().ok()
                    } else {
                        None
                    }
                });
                if let Some(hex_str) = hex_attr
                    && let Ok(bytes) = hex::decode(hex_str)
                    && let Ok(pkt) =
                        ibc_proto::ibc::core::channel::v2::Packet::decode(bytes.as_slice())
                    && client_field(&pkt) == client_id.as_str()
                {
                    sequences.insert(PacketSequence(pkt.sequence));
                }
            }
        }

        if response.txs.len() < usize::from(per_page) {
            break;
        }
        page += 1;
    }

    let mut result: Vec<_> = sequences.into_iter().collect();
    result.sort_unstable();
    Ok(result)
}

#[async_trait]
impl<S: CosmosSigner> PacketStateQuery for CosmosChain<S> {
    #[instrument(skip_all, name = "query_packet_commitment", fields(chain = %self.chain_label(), seq = %sequence))]
    async fn query_packet_commitment(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        height: &Self::Height,
    ) -> Result<(Option<PacketCommitment>, MerkleProof)> {
        let query_height = proof_query_height(*height)?;
        let response = query_abci(
            &self.rpc_client,
            &self.rpc_guard,
            IBC_STORE_PATH,
            ibc_v2_key(
                client_id.as_str(),
                COMMITMENT_DISCRIMINATOR,
                sequence.into(),
            ),
            Some(query_height),
            true,
        )
        .await?;

        let proof = extract_proof(&response)?;
        let commitment = if response.value.is_empty() {
            None
        } else {
            Some(PacketCommitment(response.value))
        };
        Ok((commitment, proof))
    }

    #[instrument(skip_all, name = "query_packet_receipt", fields(chain = %self.chain_label(), seq = %sequence))]
    async fn query_packet_receipt(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        height: &Self::Height,
    ) -> Result<(Option<PacketReceipt>, MerkleProof)> {
        let query_height = proof_query_height(*height)?;
        let response = query_abci(
            &self.rpc_client,
            &self.rpc_guard,
            IBC_STORE_PATH,
            ibc_v2_key(client_id.as_str(), RECEIPT_DISCRIMINATOR, sequence.into()),
            Some(query_height),
            true,
        )
        .await?;

        let proof = extract_proof(&response)?;
        let receipt = if response.value.is_empty() {
            None
        } else {
            Some(PacketReceipt)
        };
        Ok((receipt, proof))
    }

    #[instrument(skip_all, name = "query_commitment_sequences", fields(chain = %self.chain_label(), client_id = %client_id))]
    async fn query_commitment_sequences(
        &self,
        client_id: &Self::ClientId,
        _height: &Self::Height,
    ) -> Result<Vec<PacketSequence>> {
        paginated_packet_sequences(self, "send_packet", client_id, |pkt| &pkt.source_client).await
    }

    #[instrument(skip_all, fields(chain = %self.chain_label(), client_id = %client_id))]
    async fn query_ack_sequences(
        &self,
        client_id: &Self::ClientId,
        _height: &Self::Height,
    ) -> Result<Vec<PacketSequence>> {
        paginated_packet_sequences(self, "write_acknowledgement", client_id, |pkt| {
            &pkt.destination_client
        })
        .await
    }

    #[instrument(skip_all, name = "query_packet_ack", fields(chain = %self.chain_label(), seq = %sequence))]
    async fn query_packet_acknowledgement(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        height: &Self::Height,
    ) -> Result<(Option<PacketAcknowledgement>, MerkleProof)> {
        let query_height = proof_query_height(*height)?;
        let response = query_abci(
            &self.rpc_client,
            &self.rpc_guard,
            IBC_STORE_PATH,
            ibc_v2_key(client_id.as_str(), ACK_DISCRIMINATOR, sequence.into()),
            Some(query_height),
            true,
        )
        .await?;

        let proof = extract_proof(&response)?;
        let ack = if response.value.is_empty() {
            None
        } else {
            Some(PacketAcknowledgement(response.value))
        };
        Ok((ack, proof))
    }

    fn commitment_to_membership_entry(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        commitment: &PacketCommitment,
        proof: &MerkleProof,
    ) -> Option<mercury_core::MembershipProofEntry> {
        Some(mercury_core::MembershipProofEntry {
            path: vec![
                b"ibc".to_vec(),
                ibc_v2_key(
                    client_id.as_str(),
                    COMMITMENT_DISCRIMINATOR,
                    sequence.into(),
                ),
            ],
            value: commitment.0.clone(),
            proof: proof.proof_bytes.clone(),
        })
    }

    fn ack_to_membership_entry(
        &self,
        client_id: &Self::ClientId,
        sequence: PacketSequence,
        ack: &PacketAcknowledgement,
        proof: &MerkleProof,
    ) -> Option<mercury_core::MembershipProofEntry> {
        Some(mercury_core::MembershipProofEntry {
            path: vec![
                b"ibc".to_vec(),
                ibc_v2_key(client_id.as_str(), ACK_DISCRIMINATOR, sequence.into()),
            ],
            value: ack.0.clone(),
            proof: proof.proof_bytes.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_query_height_subtracts_one() {
        let h = TmHeight::try_from(10u64).unwrap();
        let result = proof_query_height(h).unwrap();
        assert_eq!(result.value(), 9);
    }

    #[test]
    fn proof_query_height_at_one() {
        let h = TmHeight::try_from(1u64).unwrap();
        let result = proof_query_height(h).unwrap();
        assert_eq!(result.value(), 0);
    }

    #[test]
    fn proof_query_height_at_two() {
        let h = TmHeight::try_from(2u64).unwrap();
        let result = proof_query_height(h).unwrap();
        assert_eq!(result.value(), 1);
    }

    #[test]
    fn ibc_v2_key_construction() {
        let key = ibc_v2_key("07-tendermint-0", 1, 42);
        let client_bytes = b"07-tendermint-0";
        assert!(key.starts_with(client_bytes));
        assert_eq!(key[client_bytes.len()], 1);
        let seq_bytes = &key[client_bytes.len() + 1..];
        assert_eq!(seq_bytes.len(), 8);
        assert_eq!(u64::from_be_bytes(seq_bytes.try_into().unwrap()), 42);
    }

    #[test]
    fn ibc_v2_key_different_discriminators() {
        let key_commit = ibc_v2_key("client-0", 1, 1);
        let key_receipt = ibc_v2_key("client-0", 2, 1);
        let key_ack = ibc_v2_key("client-0", 3, 1);
        assert_ne!(key_commit, key_receipt);
        assert_ne!(key_receipt, key_ack);
    }

    #[test]
    fn ibc_v2_key_sequence_encoding() {
        let key1 = ibc_v2_key("c", 1, 0);
        let key2 = ibc_v2_key("c", 1, u64::MAX);
        assert_eq!(key1.len(), key2.len());
    }
}
