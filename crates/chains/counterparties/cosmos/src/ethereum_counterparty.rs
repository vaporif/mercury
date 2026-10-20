use std::time::Duration;

use async_trait::async_trait;
use mercury_chain_traits::builders::{
    ClientMessageBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder, PacketMessageBuilder,
    UpdateClientOutput,
};
use mercury_chain_traits::queries::{ClientQuery, MisbehaviourQuery};
use mercury_core::error::Result;

use mercury_cosmos::client_types::CosmosClientState;
use mercury_cosmos::keys::CosmosSigner;
use mercury_cosmos::types::{CosmosMessage, CosmosPacket, to_any};

use mercury_ethereum::builders::{CreateClientPayload, UpdateClientPayload};
use mercury_ethereum::chain::EthereumChain;
use mercury_ethereum::types::{
    EvmAcknowledgement, EvmClientState, EvmCommitmentProof, EvmHeight, EvmPacket,
};

use ethereum_light_client::membership::MembershipProof;
use ethereum_types::execution::{account_proof::AccountProof, storage_proof::StorageProof};

use ethereum_light_client::client_state::ClientState as EthClientState;
use ibc_proto::ibc::core::channel::v2::{
    self as channel, MsgAcknowledgement, MsgRecvPacket, MsgTimeout, Packet as V2Packet,
};
use ibc_proto::ibc::core::client::v1::{Height as ProtoHeight, MsgCreateClient, MsgUpdateClient};
use ibc_proto::ibc::core::client::v2::MsgRegisterCounterparty;
use ibc_proto::ibc::lightclients::wasm::v1::{
    ClientMessage as WasmClientMessage, ClientState as WasmClientState,
    ConsensusState as WasmConsensusState,
};
use prost::Message as _;

use crate::wrapper::CosmosAdapter;

const BEACON_TRUSTING_PERIOD: Duration = Duration::from_hours(24);

impl<S: CosmosSigner> CosmosAdapter<S> {
    const fn effective_proof_height(&self, proof_height: &EvmHeight) -> u64 {
        if self.0.config.mock_proofs {
            0
        } else {
            proof_height.0
        }
    }
}

/// JSON-serialize an EVM proof into the `MembershipProof` format that the
/// WASM ethereum LC expects (`serde_json::from_slice`, not ABI).
fn encode_evm_proof_json(proof: &EvmCommitmentProof) -> Result<Vec<u8>> {
    let membership_proof = MembershipProof {
        account_proof: AccountProof {
            storage_root: proof.storage_root,
            proof: proof
                .account_proof
                .iter()
                .map(|b| alloy::primitives::Bytes::copy_from_slice(b))
                .collect(),
        },
        storage_proof: StorageProof {
            key: proof.storage_key,
            value: proof.storage_value,
            proof: proof
                .storage_proof
                .iter()
                .map(|b| alloy::primitives::Bytes::copy_from_slice(b))
                .collect(),
        },
    };
    Ok(serde_json::to_vec(&membership_proof)?)
}

#[async_trait]
impl<S: CosmosSigner> ClientQuery<EthereumChain> for CosmosAdapter<S> {
    async fn query_client_state(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Self::ClientState> {
        self.0.query_client_state(client_id, height).await
    }

    async fn query_consensus_state(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &EvmHeight,
        query_height: &Self::Height,
    ) -> Result<Self::ConsensusState> {
        // The consensus height for an Ethereum light client is a slot number.
        // We convert it to a Tendermint height for the gRPC query.
        let tm_consensus_height = tendermint::block::Height::try_from(consensus_height.0)
            .map_err(|e| eyre::eyre!("invalid consensus height: {e}"))?;
        self.0
            .query_consensus_state(client_id, &tm_consensus_height, query_height)
            .await
    }

    fn trusting_period(client_state: &Self::ClientState) -> Option<Duration> {
        match client_state {
            CosmosClientState::Wasm(_) => Some(BEACON_TRUSTING_PERIOD),
            CosmosClientState::Tendermint(_) | CosmosClientState::Attestations(_) => {
                tracing::warn!("unexpected non-Wasm client state for Ethereum counterparty");
                None
            }
        }
    }

    fn client_latest_height(client_state: &Self::ClientState) -> EvmHeight {
        match client_state {
            CosmosClientState::Wasm(cs) => cs.latest_height.as_ref().map_or_else(
                || {
                    tracing::warn!("WASM client state missing latest_height, defaulting to 0");
                    EvmHeight(0)
                },
                |h| EvmHeight(h.revision_height),
            ),
            CosmosClientState::Tendermint(_) | CosmosClientState::Attestations(_) => {
                tracing::warn!("unexpected non-Wasm client state for Ethereum counterparty");
                EvmHeight(0)
            }
        }
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientMessageBuilder<EthereumChain> for CosmosAdapter<S> {
    type CreateClientPayload = CreateClientPayload;
    type UpdateClientPayload = UpdateClientPayload;

    async fn build_create_client_message(
        &self,
        payload: CreateClientPayload,
    ) -> Result<CosmosMessage> {
        let signer = self.0.signer.account_address()?;

        let eth_cs: EthClientState = serde_json::from_slice(&payload.client_state)
            .map_err(|e| eyre::eyre!("deserializing Beacon client state: {e}"))?;

        let checksum_hex = self.0.config.wasm_checksum.as_ref().ok_or_else(|| {
            eyre::eyre!("wasm_checksum not configured — required for Beacon light client creation")
        })?;
        let checksum =
            hex::decode(checksum_hex).map_err(|e| eyre::eyre!("decoding wasm_checksum: {e}"))?;

        let wasm_client_state = WasmClientState {
            data: payload.client_state,
            checksum,
            latest_height: Some(ProtoHeight {
                revision_number: 0,
                revision_height: eth_cs.latest_slot,
            }),
        };

        let wasm_consensus_state = WasmConsensusState {
            data: payload.consensus_state,
        };

        let msg = MsgCreateClient {
            client_state: Some(ibc_proto::google::protobuf::Any {
                type_url: "/ibc.lightclients.wasm.v1.ClientState".to_string(),
                value: wasm_client_state.encode_to_vec(),
            }),
            consensus_state: Some(ibc_proto::google::protobuf::Any {
                type_url: "/ibc.lightclients.wasm.v1.ConsensusState".to_string(),
                value: wasm_consensus_state.encode_to_vec(),
            }),
            signer,
        };

        Ok(to_any(&msg))
    }

    async fn build_update_client_message(
        &self,
        client_id: &Self::ClientId,
        payload: UpdateClientPayload,
    ) -> Result<UpdateClientOutput<CosmosMessage>> {
        let signer = self.0.signer.account_address()?;

        let messages = payload
            .headers
            .into_iter()
            .map(|header_bytes| {
                let wasm_client_message = WasmClientMessage { data: header_bytes };
                let msg = MsgUpdateClient {
                    client_id: client_id.to_string(),
                    client_message: Some(ibc_proto::google::protobuf::Any {
                        type_url: "/ibc.lightclients.wasm.v1.ClientMessage".to_string(),
                        value: wasm_client_message.encode_to_vec(),
                    }),
                    signer: signer.clone(),
                };
                to_any(&msg)
            })
            .collect();

        Ok(UpdateClientOutput::messages_only(messages))
    }

    async fn build_register_counterparty_message(
        &self,
        client_id: &Self::ClientId,
        counterparty_client_id: &<EthereumChain as mercury_chain_traits::types::ChainTypes>::ClientId,
        counterparty_merkle_prefix: mercury_core::MerklePrefix,
    ) -> Result<CosmosMessage> {
        let signer = self.0.signer.account_address()?;

        let msg = MsgRegisterCounterparty {
            client_id: client_id.to_string(),
            counterparty_merkle_prefix: counterparty_merkle_prefix.0,
            counterparty_client_id: counterparty_client_id.to_string(),
            signer,
        };

        Ok(to_any(&msg))
    }
}

fn evm_packet_to_v2(packet: &EvmPacket) -> V2Packet {
    V2Packet {
        sequence: packet.sequence.into(),
        source_client: packet.source_client.clone(),
        destination_client: packet.dest_client.clone(),
        timeout_timestamp: packet.timeout_timestamp.into(),
        payloads: packet
            .payloads
            .iter()
            .map(|p| channel::Payload {
                source_port: p.source_port.0.clone(),
                destination_port: p.dest_port.0.clone(),
                version: p.version.clone(),
                encoding: p.encoding.clone(),
                value: p.value.clone(),
            })
            .collect(),
    }
}

#[async_trait]
impl<S: CosmosSigner> PacketMessageBuilder<EthereumChain> for CosmosAdapter<S> {
    async fn build_receive_packet_message(
        &self,
        packet: &EvmPacket,
        proof: EvmCommitmentProof,
        proof_height: EvmHeight,
        revision: u64,
    ) -> Result<CosmosMessage> {
        let height = self.effective_proof_height(&proof_height);
        let msg = MsgRecvPacket {
            packet: Some(evm_packet_to_v2(packet)),
            proof_commitment: encode_evm_proof_json(&proof)?,
            proof_height: Some(ProtoHeight {
                revision_number: revision,
                revision_height: height,
            }),
            signer: self.0.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }

    async fn build_ack_packet_message(
        &self,
        packet: &EvmPacket,
        ack: &EvmAcknowledgement,
        proof: EvmCommitmentProof,
        proof_height: EvmHeight,
        revision: u64,
    ) -> Result<CosmosMessage> {
        let height = self.effective_proof_height(&proof_height);
        let acknowledgement =
            channel::Acknowledgement::decode(ack.0.as_slice()).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "ack proto decode failed, treating raw bytes as single app-ack");
                channel::Acknowledgement {
                    app_acknowledgements: vec![ack.0.clone()],
                }
            });
        let msg = MsgAcknowledgement {
            packet: Some(evm_packet_to_v2(packet)),
            acknowledgement: Some(acknowledgement),
            proof_acked: encode_evm_proof_json(&proof)?,
            proof_height: Some(ProtoHeight {
                revision_number: revision,
                revision_height: height,
            }),
            signer: self.0.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &CosmosPacket,
        proof: EvmCommitmentProof,
        proof_height: EvmHeight,
        revision: u64,
    ) -> Result<CosmosMessage> {
        use mercury_cosmos::builders::cosmos_packet_to_v2;
        let height = self.effective_proof_height(&proof_height);
        let msg = MsgTimeout {
            packet: Some(cosmos_packet_to_v2(packet)),
            proof_unreceived: encode_evm_proof_json(&proof)?,
            proof_height: Some(ProtoHeight {
                revision_number: revision,
                revision_height: height,
            }),
            signer: self.0.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourDetector<EthereumChain> for CosmosAdapter<S> {
    type UpdateHeader = mercury_cosmos::misbehaviour::OnChainTmConsensusState;
    type MisbehaviourEvidence = mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence;
    type CounterpartyClientState = EvmClientState;

    #[tracing::instrument(skip_all, name = "cosmos_check_misbehaviour_for_eth")]
    async fn check_for_misbehaviour(
        &self,
        _client_id: &<EthereumChain as mercury_chain_traits::types::ChainTypes>::ClientId,
        on_chain: &mercury_cosmos::misbehaviour::OnChainTmConsensusState,
        client_state: &EvmClientState,
    ) -> Result<Option<mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence>> {
        let expected = mercury_cosmos::misbehaviour::expected_consensus_state_at_height(
            &self.0,
            on_chain.height,
        )
        .await?;

        if expected.matches_fields(on_chain) {
            return Ok(None);
        }

        tracing::error!(
            height = %on_chain.height,
            timestamp_match = %(expected.timestamp_nanos == on_chain.timestamp_nanos),
            root_match = %(expected.root == on_chain.root),
            validators_hash_match = %(expected.next_validators_hash == on_chain.next_validators_hash),
            "MISBEHAVIOUR DETECTED: consensus state mismatch (Cosmos→Ethereum)"
        );

        let decoded = mercury_ethereum::queries::decode_client_state(&client_state.0)
            .ok_or_else(|| eyre::eyre!("failed to decode EvmClientState for trusted height"))?;
        let trusted_height =
            tendermint::block::Height::try_from(decoded.latestHeight.revisionHeight)?;

        let payload = mercury_cosmos::misbehaviour::build_corrective_update_payload(
            &self.0,
            trusted_height,
            on_chain.height,
        )
        .await?;

        Ok(Some(
            mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence::CorrectiveUpdate { payload },
        ))
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourQuery<EthereumChain> for CosmosAdapter<S> {
    type CounterpartyUpdateHeader = ();

    async fn query_consensus_state_heights(
        &self,
        _client_id: &Self::ClientId,
    ) -> Result<Vec<EvmHeight>> {
        Ok(vec![])
    }

    async fn query_update_client_header(
        &self,
        _client_id: &Self::ClientId,
        _consensus_height: &EvmHeight,
    ) -> Result<Option<()>> {
        Ok(None)
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourMessageBuilder<EthereumChain> for CosmosAdapter<S> {
    type MisbehaviourEvidence = ();

    async fn build_misbehaviour_message(
        &self,
        _client_id: &Self::ClientId,
        _evidence: (),
    ) -> Result<CosmosMessage> {
        eyre::bail!("beacon chain misbehaviour not yet supported")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{B256, U256};
    use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};
    use mercury_cosmos::keys::Secp256k1KeyPair;
    use mercury_ethereum::types::{EvmPayload, ProofHeight};

    type Adapter = CosmosAdapter<Secp256k1KeyPair>;

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

    fn attestations() -> CosmosClientState {
        let any = ibc_proto::google::protobuf::Any {
            type_url: "/ibc.lightclients.attestations.v1.ClientState".into(),
            value: vec![],
        };
        CosmosClientState::decode_any(&any).unwrap()
    }

    fn latest(cs: &CosmosClientState) -> u64 {
        <Adapter as ClientQuery<EthereumChain>>::client_latest_height(cs).0
    }

    fn trusting(cs: &CosmosClientState) -> Option<Duration> {
        <Adapter as ClientQuery<EthereumChain>>::trusting_period(cs)
    }

    #[test]
    fn latest_height_from_wasm_client_state() {
        assert_eq!(latest(&wasm(Some(8_000_000))), 8_000_000);
        assert_eq!(latest(&wasm(None)), 0);
    }

    #[test]
    fn latest_height_non_wasm_defaults_to_zero() {
        assert_eq!(latest(&attestations()), 0);
    }

    #[test]
    fn trusting_period_only_for_wasm() {
        assert_eq!(trusting(&wasm(Some(1))), Some(BEACON_TRUSTING_PERIOD));
        assert_eq!(trusting(&attestations()), None);
    }

    #[test]
    fn evm_packet_to_v2_maps_fields() {
        let packet = EvmPacket {
            source_client: "client-0".into(),
            dest_client: "08-wasm-0".into(),
            sequence: PacketSequence(3),
            timeout_timestamp: TimeoutTimestamp(777),
            payloads: vec![EvmPayload {
                source_port: Port("transfer".into()),
                dest_port: Port("dest-port".into()),
                version: "ics20-1".into(),
                encoding: "application/x-solidity-abi".into(),
                value: vec![9, 8],
            }],
        };
        let v2 = evm_packet_to_v2(&packet);
        assert_eq!(v2.sequence, 3);
        assert_eq!(v2.source_client, "client-0");
        assert_eq!(v2.destination_client, "08-wasm-0");
        assert_eq!(v2.timeout_timestamp, 777);
        let p = &v2.payloads[0];
        assert_eq!(p.source_port, "transfer");
        assert_eq!(p.destination_port, "dest-port");
        assert_eq!(p.version, "ics20-1");
        assert_eq!(p.encoding, "application/x-solidity-abi");
        assert_eq!(p.value, vec![9, 8]);
    }

    #[test]
    fn evm_proof_json_round_trips_into_membership_proof() {
        let proof = EvmCommitmentProof {
            proof_height: ProofHeight(10),
            storage_root: B256::repeat_byte(0x11),
            account_proof: vec![vec![1, 2], vec![3]],
            storage_key: B256::repeat_byte(0x22),
            storage_value: U256::from(42u64),
            storage_proof: vec![vec![4, 5, 6]],
        };
        let json = encode_evm_proof_json(&proof).unwrap();
        let decoded: MembershipProof = serde_json::from_slice(&json).unwrap();

        assert_eq!(decoded.account_proof.storage_root, B256::repeat_byte(0x11));
        let account: Vec<Vec<u8>> = decoded
            .account_proof
            .proof
            .iter()
            .map(|b| b.to_vec())
            .collect();
        assert_eq!(account, vec![vec![1, 2], vec![3]]);
        assert_eq!(decoded.storage_proof.key, B256::repeat_byte(0x22));
        assert_eq!(decoded.storage_proof.value, U256::from(42u64));
        let storage: Vec<Vec<u8>> = decoded
            .storage_proof
            .proof
            .iter()
            .map(|b| b.to_vec())
            .collect();
        assert_eq!(storage, vec![vec![4, 5, 6]]);
    }

    #[test]
    fn evm_proof_json_empty_proofs() {
        let proof = EvmCommitmentProof {
            proof_height: ProofHeight(0),
            storage_root: B256::ZERO,
            account_proof: vec![],
            storage_key: B256::ZERO,
            storage_value: U256::ZERO,
            storage_proof: vec![],
        };
        let decoded: MembershipProof =
            serde_json::from_slice(&encode_evm_proof_json(&proof).unwrap()).unwrap();
        assert_eq!(decoded, MembershipProof::default());
    }
}
