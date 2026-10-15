use async_trait::async_trait;
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

use crate::events::ENCODED_PACKET_HEX;

use crate::chain::CosmosChain;
use crate::client_types::{CosmosClientState, CosmosConsensusState};
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

        CosmosClientState::decode_any(&any)
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

        CosmosConsensusState::decode_any(&any)
    }

    fn trusting_period(client_state: &Self::ClientState) -> Option<std::time::Duration> {
        match client_state {
            CosmosClientState::Tendermint(cs) => Some(cs.trusting_period),
            CosmosClientState::Wasm(_) | CosmosClientState::Attestations(_) => None,
        }
    }

    #[expect(clippy::expect_used, reason = "height clamped to a valid range")]
    fn client_latest_height(client_state: &Self::ClientState) -> Self::Height {
        match client_state {
            CosmosClientState::Tendermint(cs) => {
                let h = cs.latest_height.revision_height();
                if h == 0 {
                    warn!("Tendermint client state has revision_height 0, clamping to 1");
                }
                TmHeight::try_from(h.max(1)).expect("nonzero after .max(1)")
            }
            CosmosClientState::Wasm(cs) => {
                let Some(h) = cs.latest_height.as_ref() else {
                    warn!("WASM client state missing latest_height, defaulting to 1");
                    return TmHeight::from(1_u32);
                };
                if h.revision_height == 0 {
                    warn!("WASM client state has revision_height 0, clamping to 1");
                }
                TmHeight::try_from(h.revision_height.max(1)).expect("nonzero after .max(1)")
            }
            CosmosClientState::Attestations(cs) => {
                TmHeight::try_from(cs.latest_height.max(1)).expect("nonzero after .max(1)")
            }
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

    let query = Query::from(EventType::Tx).and_exists(format!("{event_kind}.{ENCODED_PACKET_HEX}"));

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

        let matched = response
            .txs
            .iter()
            .flat_map(|tx| &tx.tx_result.events)
            .filter(|event| event.kind == event_kind)
            .filter_map(|event| {
                let hex_str = event.attributes.iter().find_map(|attr| {
                    (attr.key_str().ok()? == ENCODED_PACKET_HEX).then(|| attr.value_str().ok())?
                })?;
                let bytes = hex::decode(hex_str).ok()?;
                let pkt =
                    ibc_proto::ibc::core::channel::v2::Packet::decode(bytes.as_slice()).ok()?;
                (client_field(&pkt) == client_id.as_str()).then_some(PacketSequence(pkt.sequence))
            });

        sequences.extend(matched);

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

    #[test]
    fn proof_query_height_zero_underflows() {
        let h = TmHeight::try_from(0u64).unwrap();
        assert!(proof_query_height(h).is_err());
    }

    #[test]
    fn ibc_v2_key_exact_layout() {
        let key = ibc_v2_key("ab", ACK_DISCRIMINATOR, 0x0102);
        assert_eq!(key, vec![b'a', b'b', 0x03, 0, 0, 0, 0, 0, 0, 0x01, 0x02]);
    }

    mod proofs {
        use super::*;
        use ibc_proto::ics23::{CommitmentProof, ExistenceProof, commitment_proof};
        use tendermint::merkle::proof::{ProofOp, ProofOps};
        use tendermint_rpc::endpoint::abci_query::AbciQuery;

        fn exist_proof(key: &[u8]) -> CommitmentProof {
            CommitmentProof {
                proof: Some(commitment_proof::Proof::Exist(ExistenceProof {
                    key: key.to_vec(),
                    value: b"v".to_vec(),
                    leaf: None,
                    path: vec![],
                })),
            }
        }

        fn op(data: Vec<u8>) -> ProofOp {
            ProofOp {
                field_type: "ics23:iavl".into(),
                key: vec![],
                data,
            }
        }

        fn decode(proof: &MerkleProof) -> ibc_proto::ibc::core::commitment::v1::MerkleProof {
            ibc_proto::ibc::core::commitment::v1::MerkleProof::decode(proof.proof_bytes.as_slice())
                .unwrap()
        }

        #[test]
        fn missing_proof_is_error() {
            let response = AbciQuery::default();
            let err = extract_proof(&response).unwrap_err();
            assert!(matches!(
                err.downcast_ref::<ProofError>(),
                Some(ProofError::Missing)
            ));
        }

        #[test]
        fn keeps_op_order() {
            let response = AbciQuery {
                proof: Some(ProofOps {
                    ops: vec![
                        op(exist_proof(b"iavl").encode_to_vec()),
                        op(exist_proof(b"simple").encode_to_vec()),
                    ],
                }),
                ..Default::default()
            };
            let merkle = decode(&extract_proof(&response).unwrap());
            assert_eq!(
                merkle.proofs,
                vec![exist_proof(b"iavl"), exist_proof(b"simple")]
            );
        }

        #[test]
        fn skips_undecodable_ops() {
            let response = AbciQuery {
                proof: Some(ProofOps {
                    ops: vec![
                        op(vec![0xff, 0xff, 0xff, 0xff]),
                        op(exist_proof(b"k").encode_to_vec()),
                    ],
                }),
                ..Default::default()
            };
            let merkle = decode(&extract_proof(&response).unwrap());
            assert_eq!(merkle.proofs, vec![exist_proof(b"k")]);
        }

        #[test]
        fn empty_ops_yields_empty_merkle_proof() {
            let response = AbciQuery {
                proof: Some(ProofOps { ops: vec![] }),
                ..Default::default()
            };
            assert_eq!(decode(&extract_proof(&response).unwrap()).proofs, vec![]);
        }

        #[test]
        fn fuzz_extract_proof() {
            bolero::check!()
                .with_type::<Vec<Vec<u8>>>()
                .for_each(|ops| {
                    let response = AbciQuery {
                        proof: Some(ProofOps {
                            ops: ops.iter().cloned().map(op).collect(),
                        }),
                        ..Default::default()
                    };
                    let merkle = decode(&extract_proof(&response).unwrap());
                    assert!(merkle.proofs.len() <= ops.len());
                });
        }
    }

    mod client_state {
        use super::*;
        use crate::chain::test_support::tm_client_state;
        use crate::keys::Secp256k1KeyPair;
        use ibc_proto::ibc::core::client::v1::Height as ProtoHeight;
        use ibc_proto::ibc::lightclients::wasm::v1::ClientState as WasmClientState;
        use mercury_attestor::proto::lightclient::ClientState as AttestationsClientState;

        type TestChain = CosmosChain<Secp256k1KeyPair>;

        fn wasm(latest_height: Option<u64>) -> CosmosClientState {
            CosmosClientState::Wasm(WasmClientState {
                data: vec![],
                checksum: vec![],
                latest_height: latest_height.map(|h| ProtoHeight {
                    revision_number: 0,
                    revision_height: h,
                }),
            })
        }

        fn attestations(latest_height: u64) -> CosmosClientState {
            CosmosClientState::Attestations(AttestationsClientState {
                attestor_addresses: vec![],
                min_required_sigs: 1,
                latest_height,
                is_frozen: false,
            })
        }

        fn latest(cs: &CosmosClientState) -> u64 {
            TestChain::client_latest_height(cs).value()
        }

        #[test]
        fn latest_height_tendermint() {
            assert_eq!(
                latest(&CosmosClientState::Tendermint(tm_client_state(77))),
                77
            );
        }

        #[test]
        fn latest_height_wasm() {
            assert_eq!(latest(&wasm(Some(500))), 500);
        }

        #[test]
        fn latest_height_wasm_missing_or_zero_clamps_to_one() {
            assert_eq!(latest(&wasm(None)), 1);
            assert_eq!(latest(&wasm(Some(0))), 1);
        }

        #[test]
        fn latest_height_attestations() {
            assert_eq!(latest(&attestations(42)), 42);
            assert_eq!(latest(&attestations(0)), 1);
        }

        #[test]
        fn trusting_period_only_for_tendermint() {
            let tm = tm_client_state(1);
            let expected = tm.trusting_period;
            assert_eq!(
                TestChain::trusting_period(&CosmosClientState::Tendermint(tm)),
                Some(expected)
            );
            assert_eq!(TestChain::trusting_period(&wasm(Some(1))), None);
            assert_eq!(TestChain::trusting_period(&attestations(1)), None);
        }
    }

    mod chain_queries {
        use super::*;
        use crate::chain::test_support::test_chain;
        use ibc::core::host::types::identifiers::ClientId;

        fn client_id() -> ClientId {
            "07-tendermint-3".parse().unwrap()
        }

        #[tokio::test]
        async fn commitment_membership_entry_uses_commitment_key() {
            let chain = test_chain();
            let proof = MerkleProof {
                proof_bytes: vec![9, 9],
            };
            let entry = chain
                .commitment_to_membership_entry(
                    &client_id(),
                    PacketSequence(5),
                    &PacketCommitment(vec![1, 2, 3]),
                    &proof,
                )
                .unwrap();
            assert_eq!(
                entry.path,
                vec![
                    b"ibc".to_vec(),
                    ibc_v2_key("07-tendermint-3", COMMITMENT_DISCRIMINATOR, 5)
                ]
            );
            assert_eq!(entry.value, vec![1, 2, 3]);
            assert_eq!(entry.proof, vec![9, 9]);
        }

        #[tokio::test]
        async fn ack_membership_entry_uses_ack_key() {
            let chain = test_chain();
            let proof = MerkleProof {
                proof_bytes: vec![7],
            };
            let entry = chain
                .ack_to_membership_entry(
                    &client_id(),
                    PacketSequence(8),
                    &PacketAcknowledgement(vec![4]),
                    &proof,
                )
                .unwrap();
            assert_eq!(
                entry.path,
                vec![
                    b"ibc".to_vec(),
                    ibc_v2_key("07-tendermint-3", ACK_DISCRIMINATOR, 8)
                ]
            );
            assert_eq!(entry.value, vec![4]);
            assert_eq!(entry.proof, vec![7]);
        }

        #[tokio::test]
        async fn packet_queries_at_height_zero_fail_before_rpc() {
            let chain = test_chain();
            let zero = TmHeight::try_from(0u64).unwrap();
            let seq = PacketSequence(1);
            let is_underflow = |e: eyre::Report| e.to_string().contains("underflow");
            assert!(is_underflow(
                chain
                    .query_packet_commitment(&client_id(), seq, &zero)
                    .await
                    .unwrap_err()
            ));
            assert!(is_underflow(
                chain
                    .query_packet_receipt(&client_id(), seq, &zero)
                    .await
                    .unwrap_err()
            ));
            assert!(is_underflow(
                chain
                    .query_packet_acknowledgement(&client_id(), seq, &zero)
                    .await
                    .unwrap_err()
            ));
        }
    }

    mod mocked {
        use super::*;
        use crate::chain::test_support::grpc::{GrpcReply, reply, spawn_grpc, unimplemented};
        use crate::chain::test_support::rpc::{Reply, spawn_rpc};
        use crate::chain::test_support::tm::{
            abci_reply, block_time, event, exec_result, proof_ops, status_json, tx_entry,
            tx_search_reply,
        };
        use crate::chain::test_support::{
            UNREACHABLE_ADDR, mock_chain, mock_config, test_chain_with, tm_client_state,
        };
        use ibc::core::host::types::identifiers::ClientId;
        use ibc_client_tendermint::types::ConsensusState as TmConsensusState;
        use ibc_proto::ibc::core::client::v1::{
            QueryClientStateResponse, QueryConsensusStateResponse,
        };
        use mercury_core::error::RpcError;
        use serde_json::Value;

        const CLIENT_STATE_PATH: &str = "/ibc.core.client.v1.Query/ClientState";
        const CONSENSUS_STATE_PATH: &str = "/ibc.core.client.v1.Query/ConsensusState";

        fn client_id() -> ClientId {
            "07-tendermint-3".parse().unwrap()
        }

        fn height(h: u64) -> TmHeight {
            TmHeight::try_from(h).unwrap()
        }

        // ibc-go host.PacketCommitmentKey & co: clientID || discriminator || big-endian sequence
        fn spec_key(client: &str, discriminator: u8, seq: u64) -> Vec<u8> {
            [client.as_bytes(), &[discriminator], &seq.to_be_bytes()].concat()
        }

        #[tokio::test]
        async fn chain_status_reads_latest_block_height_and_time() {
            let rpc = spawn_rpc(|_| Reply::Result(status_json(321, false, true))).await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let status = chain.query_chain_status().await.unwrap();
            assert_eq!(status.height.value(), 321);
            assert_eq!(status.timestamp, block_time(321));
            assert_eq!(rpc.calls()[0].method, "status");

            let free = query_cosmos_status(&rpc.url).await.unwrap();
            assert_eq!(free.height.value(), 321);
            assert_eq!(free.timestamp, block_time(321));
        }

        #[tokio::test]
        async fn chain_status_rpc_error_and_malformed_body_are_errors() {
            let rpc = spawn_rpc(|_| Reply::Error("boom".into())).await;
            let err = mock_chain(&rpc.url, UNREACHABLE_ADDR)
                .query_chain_status()
                .await
                .unwrap_err();
            assert!(format!("{err:#}").contains("boom"), "{err:#}");
            assert!(query_cosmos_status(&rpc.url).await.is_err());

            let rpc = spawn_rpc(|_| Reply::Raw("not json".into())).await;
            assert!(
                mock_chain(&rpc.url, UNREACHABLE_ADDR)
                    .query_chain_status()
                    .await
                    .is_err()
            );
        }

        #[tokio::test(start_paused = true)]
        async fn chain_status_hanging_node_hits_rpc_timeout() {
            let rpc = spawn_rpc(|_| Reply::Hang).await;
            let err = mock_chain(&rpc.url, UNREACHABLE_ADDR)
                .query_chain_status()
                .await
                .unwrap_err();
            assert!(
                matches!(err.downcast_ref::<RpcError>(), Some(RpcError::Timeout(_))),
                "{err:#}"
            );
        }

        #[tokio::test(start_paused = true)]
        async fn client_state_hanging_grpc_hits_rpc_timeout() {
            let grpc = spawn_grpc(|_| GrpcReply::Hang).await;
            let err = mock_chain(UNREACHABLE_ADDR, &grpc.url)
                .query_client_state(&client_id(), &height(5))
                .await
                .unwrap_err();
            assert!(
                matches!(err.downcast_ref::<RpcError>(), Some(RpcError::Timeout(_))),
                "{err:#}"
            );
        }

        #[tokio::test]
        async fn client_state_query_pins_height_and_decodes_any() {
            let grpc = spawn_grpc(|call| match call.path.as_str() {
                CLIENT_STATE_PATH => reply(&QueryClientStateResponse {
                    client_state: Some(tm_client_state(77).into()),
                    proof: vec![],
                    proof_height: None,
                }),
                _ => unimplemented(call),
            })
            .await;
            let chain = mock_chain(UNREACHABLE_ADDR, &grpc.url);
            let cs = chain
                .query_client_state(&client_id(), &height(55))
                .await
                .unwrap();
            let CosmosClientState::Tendermint(tm) = &cs else {
                panic!("expected tendermint client state, got {cs:?}");
            };
            assert_eq!(tm.latest_height.revision_height(), 77);

            let calls = grpc.calls_to(CLIENT_STATE_PATH);
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].height.as_deref(), Some("55"));
            let req: QueryClientStateRequest = calls[0].decode();
            assert_eq!(req.client_id, "07-tendermint-3");
        }

        #[tokio::test]
        async fn client_state_missing_is_stale_and_bad_any_is_error() {
            let grpc = spawn_grpc(|_| reply(&QueryClientStateResponse::default())).await;
            let chain = mock_chain(UNREACHABLE_ADDR, &grpc.url);
            let err = chain
                .query_client_state(&client_id(), &height(5))
                .await
                .unwrap_err();
            assert!(
                matches!(
                    err.downcast_ref::<QueryError>(),
                    Some(QueryError::StaleState { .. })
                ),
                "{err:#}"
            );

            let grpc = spawn_grpc(|_| {
                reply(&QueryClientStateResponse {
                    client_state: Some(ibc_proto::google::protobuf::Any {
                        type_url: "/unknown.ClientState".into(),
                        value: vec![1, 2, 3],
                    }),
                    ..Default::default()
                })
            })
            .await;
            let chain = mock_chain(UNREACHABLE_ADDR, &grpc.url);
            assert!(
                chain
                    .query_client_state(&client_id(), &height(5))
                    .await
                    .is_err()
            );
        }

        #[tokio::test]
        async fn client_state_grpc_status_error_propagates() {
            let grpc =
                spawn_grpc(|_| GrpcReply::Err(tonic::Status::not_found("client not found"))).await;
            let chain = mock_chain(UNREACHABLE_ADDR, &grpc.url);
            let err = chain
                .query_client_state(&client_id(), &height(5))
                .await
                .unwrap_err();
            assert!(format!("{err:#}").contains("client not found"), "{err:#}");
            let err = chain
                .query_consensus_state(&client_id(), &height(4), &height(5))
                .await
                .unwrap_err();
            assert!(format!("{err:#}").contains("client not found"), "{err:#}");
        }

        #[tokio::test]
        async fn consensus_state_request_uses_chain_revision_and_query_height() {
            let header = crate::chain::test_support::tm_ibc_header(40, 0x5a);
            let cons = TmConsensusState::from(header.signed_header.header);
            let expected_root = cons.root.clone();
            let grpc = spawn_grpc(move |call| match call.path.as_str() {
                CONSENSUS_STATE_PATH => reply(&QueryConsensusStateResponse {
                    consensus_state: Some(cons.clone().into()),
                    proof: vec![],
                    proof_height: None,
                }),
                _ => unimplemented(call),
            })
            .await;
            let chain = mock_chain(UNREACHABLE_ADDR, &grpc.url);
            let got = chain
                .query_consensus_state(&client_id(), &height(40), &height(90))
                .await
                .unwrap();
            let CosmosConsensusState::Tendermint(got) = got else {
                panic!("expected tendermint consensus state");
            };
            assert_eq!(got.root, expected_root);

            let call = &grpc.calls_to(CONSENSUS_STATE_PATH)[0];
            assert_eq!(call.height.as_deref(), Some("90"));
            let req: QueryConsensusStateRequest = call.decode();
            assert_eq!(req.client_id, "07-tendermint-3");
            // "testchain-1" encodes revision 1
            assert_eq!(req.revision_number, 1);
            assert_eq!(req.revision_height, 40);
            assert!(!req.latest_height);
        }

        #[tokio::test]
        async fn consensus_state_missing_is_stale() {
            let grpc = spawn_grpc(|_| reply(&QueryConsensusStateResponse::default())).await;
            let chain = mock_chain(UNREACHABLE_ADDR, &grpc.url);
            let err = chain
                .query_consensus_state(&client_id(), &height(4), &height(5))
                .await
                .unwrap_err();
            assert!(matches!(
                err.downcast_ref::<QueryError>(),
                Some(QueryError::StaleState { .. })
            ));
        }

        fn abci_params(rpc: &crate::chain::test_support::rpc::MockRpc) -> Value {
            let calls = rpc.calls_to("abci_query");
            assert_eq!(calls.len(), 1);
            calls[0].clone()
        }

        #[tokio::test]
        async fn packet_commitment_queries_spec_key_one_below_proof_height() {
            let key = spec_key("07-tendermint-3", 1, 9);
            let rpc =
                spawn_rpc(|_| abci_reply(vec![0xc0; 32], Some(proof_ops(&[b"iavl", b"ibc"]))))
                    .await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let (commitment, proof) = chain
                .query_packet_commitment(&client_id(), PacketSequence(9), &height(20))
                .await
                .unwrap();
            assert_eq!(commitment.unwrap().0, vec![0xc0; 32]);
            let merkle = ibc_proto::ibc::core::commitment::v1::MerkleProof::decode(
                proof.proof_bytes.as_slice(),
            )
            .unwrap();
            assert_eq!(merkle.proofs.len(), 2);

            let params = abci_params(&rpc);
            assert_eq!(params["path"], "store/ibc/key");
            assert_eq!(params["data"], hex::encode_upper(&key));
            assert_eq!(params["height"], "19");
            assert_eq!(params["prove"], true);
        }

        #[tokio::test]
        async fn packet_commitment_absent_value_is_none_with_proof() {
            let rpc = spawn_rpc(|_| abci_reply(vec![], Some(proof_ops(&[b"nonexist"])))).await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let (commitment, proof) = chain
                .query_packet_commitment(&client_id(), PacketSequence(1), &height(2))
                .await
                .unwrap();
            assert!(commitment.is_none());
            assert_ne!(proof.proof_bytes, Vec::<u8>::new());
        }

        #[tokio::test]
        async fn packet_receipt_uses_receipt_discriminator() {
            let rpc = spawn_rpc(|call| {
                let present =
                    call.params["data"] == hex::encode_upper(spec_key("07-tendermint-3", 2, 4));
                abci_reply(
                    if present { vec![1] } else { vec![] },
                    Some(proof_ops(&[b"k"])),
                )
            })
            .await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let (receipt, _) = chain
                .query_packet_receipt(&client_id(), PacketSequence(4), &height(8))
                .await
                .unwrap();
            assert!(receipt.is_some(), "mock only answers the spec receipt key");
            let (receipt, _) = chain
                .query_packet_receipt(&client_id(), PacketSequence(5), &height(8))
                .await
                .unwrap();
            assert!(receipt.is_none());
            assert_eq!(rpc.calls_to("abci_query")[0]["height"], "7");
        }

        #[tokio::test]
        async fn packet_ack_uses_ack_discriminator() {
            let rpc = spawn_rpc(|call| {
                let present =
                    call.params["data"] == hex::encode_upper(spec_key("07-tendermint-3", 3, 6));
                abci_reply(
                    if present { vec![0xac] } else { vec![] },
                    Some(proof_ops(&[b"k"])),
                )
            })
            .await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let (ack, _) = chain
                .query_packet_acknowledgement(&client_id(), PacketSequence(6), &height(3))
                .await
                .unwrap();
            assert_eq!(ack.unwrap().0, vec![0xac]);
        }

        #[tokio::test]
        async fn packet_queries_without_proof_are_proof_missing() {
            let rpc = spawn_rpc(|_| abci_reply(vec![1], None)).await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let is_missing = |e: eyre::Report| {
                matches!(e.downcast_ref::<ProofError>(), Some(ProofError::Missing))
            };
            let seq = PacketSequence(1);
            assert!(is_missing(
                chain
                    .query_packet_commitment(&client_id(), seq, &height(3))
                    .await
                    .unwrap_err()
            ));
            assert!(is_missing(
                chain
                    .query_packet_receipt(&client_id(), seq, &height(3))
                    .await
                    .unwrap_err()
            ));
            assert!(is_missing(
                chain
                    .query_packet_acknowledgement(&client_id(), seq, &height(3))
                    .await
                    .unwrap_err()
            ));
        }

        fn packet_hex(seq: u64, source: &str, dest: &str) -> String {
            hex::encode(
                ibc_proto::ibc::core::channel::v2::Packet {
                    sequence: seq,
                    source_client: source.into(),
                    destination_client: dest.into(),
                    timeout_timestamp: 0,
                    payloads: vec![],
                }
                .encode_to_vec(),
            )
        }

        fn packet_tx(kind: &str, seq: u64, source: &str, dest: &str) -> Value {
            tx_entry(
                tendermint::Hash::Sha256([u8::try_from(seq % 256).unwrap(); 32]),
                10,
                exec_result(
                    0,
                    "",
                    vec![event(
                        kind,
                        &[(ENCODED_PACKET_HEX, &packet_hex(seq, source, dest))],
                    )],
                ),
            )
        }

        #[tokio::test]
        async fn commitment_sequences_filter_by_source_client_dedup_and_sort() {
            let ours = "07-tendermint-3";
            let rpc = spawn_rpc(move |_| {
                tx_search_reply(vec![
                    packet_tx("send_packet", 7, ours, "x"),
                    packet_tx("send_packet", 2, ours, "x"),
                    packet_tx("send_packet", 7, ours, "x"),
                    packet_tx("send_packet", 5, "07-tendermint-9", "x"),
                    // right client but wrong event kind
                    packet_tx("write_acknowledgement", 4, ours, "x"),
                ])
            })
            .await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let seqs = chain
                .query_commitment_sequences(&client_id(), &height(1))
                .await
                .unwrap();
            assert_eq!(seqs, vec![PacketSequence(2), PacketSequence(7)]);

            let params = &rpc.calls_to("tx_search")[0];
            assert_eq!(
                params["query"],
                "tm.event = 'Tx' AND send_packet.encoded_packet_hex EXISTS"
            );
            assert_eq!(params["page"], "1");
            assert_eq!(params["per_page"], "100");
            assert_eq!(params["order_by"], "asc");
        }

        #[tokio::test]
        async fn ack_sequences_filter_by_destination_client() {
            let ours = "07-tendermint-3";
            let rpc = spawn_rpc(move |_| {
                tx_search_reply(vec![
                    packet_tx("write_acknowledgement", 3, "other", ours),
                    packet_tx("write_acknowledgement", 8, ours, "other"),
                ])
            })
            .await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let seqs = chain
                .query_ack_sequences(&client_id(), &height(1))
                .await
                .unwrap();
            assert_eq!(seqs, vec![PacketSequence(3)]);
            assert_eq!(
                rpc.calls_to("tx_search")[0]["query"],
                "tm.event = 'Tx' AND write_acknowledgement.encoded_packet_hex EXISTS"
            );
        }

        #[tokio::test]
        async fn sequences_skip_undecodable_packets() {
            let rpc = spawn_rpc(|_| {
                tx_search_reply(vec![
                    tx_entry(
                        tendermint::Hash::Sha256([1; 32]),
                        1,
                        exec_result(
                            0,
                            "",
                            vec![event("send_packet", &[(ENCODED_PACKET_HEX, "zz")])],
                        ),
                    ),
                    tx_entry(
                        tendermint::Hash::Sha256([2; 32]),
                        1,
                        exec_result(
                            0,
                            "",
                            vec![event("send_packet", &[(ENCODED_PACKET_HEX, "ffff")])],
                        ),
                    ),
                    packet_tx("send_packet", 11, "07-tendermint-3", "x"),
                ])
            })
            .await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let seqs = chain
                .query_commitment_sequences(&client_id(), &height(1))
                .await
                .unwrap();
            assert_eq!(seqs, vec![PacketSequence(11)]);
        }

        #[tokio::test]
        async fn sequences_follow_full_pages_until_a_short_one() {
            let ours = "07-tendermint-3";
            let rpc = spawn_rpc(move |call| {
                let page: u64 = call.params["page"].as_str().unwrap().parse().unwrap();
                let count = if page == 1 { 100 } else { 3 };
                let base = (page - 1) * 100;
                tx_search_reply(
                    (1..=count)
                        .map(|i| packet_tx("send_packet", base + i, ours, "x"))
                        .collect(),
                )
            })
            .await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let seqs = chain
                .query_commitment_sequences(&client_id(), &height(1))
                .await
                .unwrap();
            assert_eq!(seqs.len(), 103);
            assert_eq!(seqs.first(), Some(&PacketSequence(1)));
            assert_eq!(seqs.last(), Some(&PacketSequence(103)));
            let pages: Vec<_> = rpc
                .calls_to("tx_search")
                .iter()
                .map(|p| p["page"].clone())
                .collect();
            assert_eq!(pages, vec!["1", "2"]);
        }

        // Port of CometBFT v0.38 rpc/core/env.go `validatePage` + tx_search
        // slicing: a page past the last one is a JSON-RPC internal error.
        fn cometbft_tx_search(
            total: u64,
            call: &crate::chain::test_support::rpc::RpcCall,
        ) -> Reply {
            let per_page: u64 = call.params["per_page"].as_str().unwrap().parse().unwrap();
            let page: u64 = call.params["page"].as_str().unwrap().parse().unwrap();
            let pages = (total.saturating_sub(1) / per_page + 1).max(1);
            if page == 0 || page > pages {
                return Reply::Error(format!(
                    "page should be within [1, {pages}] range, given {page}"
                ));
            }
            let skip = (page - 1) * per_page;
            let size = per_page.min(total - skip);
            let txs = (skip + 1..=skip + size)
                .map(|seq| packet_tx("send_packet", seq, "07-tendermint-3", "x"))
                .collect::<Vec<_>>();
            Reply::Result(serde_json::json!({"txs": txs, "total_count": total.to_string()}))
        }

        #[tokio::test]
        #[ignore = "bug: tx_search requests page N+1 after an exactly-full page; CometBFT rejects it"]
        async fn sequences_exactly_one_full_page_against_cometbft_pagination() {
            let rpc = spawn_rpc(|call| cometbft_tx_search(100, call)).await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let seqs = chain
                .query_commitment_sequences(&client_id(), &height(1))
                .await;
            let pages: Vec<_> = rpc
                .calls_to("tx_search")
                .iter()
                .map(|p| p["page"].clone())
                .collect();
            let seqs = seqs.unwrap_or_else(|e| panic!("pages requested {pages:?}: {e:?}"));
            assert_eq!(seqs.len(), 100);
        }

        #[tokio::test]
        async fn sequences_rpc_error_propagates() {
            let rpc = spawn_rpc(|_| Reply::Error("index disabled".into())).await;
            let chain = test_chain_with(mock_config(&rpc.url, UNREACHABLE_ADDR));
            assert!(
                chain
                    .query_commitment_sequences(&client_id(), &height(1))
                    .await
                    .is_err()
            );
        }
    }
}
