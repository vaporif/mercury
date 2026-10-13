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

use mercury_solana::chain::{SolanaChain, SolanaMisbehaviourEvidence};
use mercury_solana::types::{
    SolanaAcknowledgement, SolanaClientId, SolanaClientState, SolanaCommitmentProof,
    SolanaCreateClientPayload, SolanaHeight, SolanaPacket, SolanaUpdateClientPayload,
};

use ibc_proto::ibc::core::channel::v2::{
    self as channel, MsgAcknowledgement, MsgRecvPacket, MsgTimeout, Packet as V2Packet,
};
use ibc_proto::ibc::core::client::v1::{Height as ProtoHeight, MsgCreateClient, MsgUpdateClient};
use ibc_proto::ibc::core::client::v2::MsgRegisterCounterparty;
use ibc_proto::ibc::lightclients::wasm::v1::{
    ClientMessage as WasmClientMessage, ClientState as WasmClientState,
    ConsensusState as WasmConsensusState,
};
use mercury_attestor::proto::lightclient as attestations;
use mercury_attestor::{CommitmentType, abi_encode_packet};
use prost::Message as _;

use crate::wrapper::CosmosAdapter;

const SOLANA_TRUSTING_PERIOD: Duration = Duration::from_hours(24);
const NANOS_PER_SECOND: u64 = 1_000_000_000;

#[async_trait]
impl<S: CosmosSigner> ClientQuery<SolanaChain> for CosmosAdapter<S> {
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
        consensus_height: &SolanaHeight,
        query_height: &Self::Height,
    ) -> Result<Self::ConsensusState> {
        let tm_consensus_height = tendermint::block::Height::try_from(consensus_height.0)
            .map_err(|e| eyre::eyre!("invalid consensus height: {e}"))?;
        self.0
            .query_consensus_state(client_id, &tm_consensus_height, query_height)
            .await
    }

    fn trusting_period(client_state: &Self::ClientState) -> Option<Duration> {
        match client_state {
            CosmosClientState::Wasm(_) => Some(SOLANA_TRUSTING_PERIOD),
            // Attested state doesn't expire.
            CosmosClientState::Attestations(_) => None,
            CosmosClientState::Tendermint(_) => {
                tracing::warn!("unexpected Tendermint client state for Solana counterparty");
                None
            }
        }
    }

    fn client_latest_height(client_state: &Self::ClientState) -> SolanaHeight {
        match client_state {
            CosmosClientState::Wasm(cs) => cs.latest_height.as_ref().map_or_else(
                || {
                    tracing::warn!("WASM client state missing latest_height, defaulting to 0");
                    SolanaHeight(0)
                },
                |h| SolanaHeight(h.revision_height),
            ),
            CosmosClientState::Attestations(cs) => SolanaHeight(cs.latest_height),
            CosmosClientState::Tendermint(_) => {
                tracing::warn!("unexpected Tendermint client state for Solana counterparty");
                SolanaHeight(0)
            }
        }
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientMessageBuilder<SolanaChain> for CosmosAdapter<S> {
    type CreateClientPayload = SolanaCreateClientPayload;
    type UpdateClientPayload = SolanaUpdateClientPayload;

    async fn build_create_client_message(
        &self,
        payload: SolanaCreateClientPayload,
    ) -> Result<CosmosMessage> {
        let signer = self.0.signer.account_address()?;

        let (client_state, consensus_state) = match payload {
            SolanaCreateClientPayload::Dummy {
                client_state,
                consensus_state,
            } => (client_state, consensus_state),
            SolanaCreateClientPayload::Attested {
                attestor_addresses,
                min_required_sigs,
                height,
                timestamp_secs,
            } => {
                let client_state = attestations::ClientState {
                    attestor_addresses,
                    min_required_sigs,
                    latest_height: height,
                    is_frozen: false,
                };
                let consensus_state = attestations::ConsensusState {
                    timestamp: timestamp_secs.saturating_mul(NANOS_PER_SECOND),
                };
                let msg = MsgCreateClient {
                    client_state: Some(any(attestations::CLIENT_STATE_TYPE_URL, &client_state)),
                    consensus_state: Some(any(
                        attestations::CONSENSUS_STATE_TYPE_URL,
                        &consensus_state,
                    )),
                    signer,
                };
                return Ok(to_any(&msg));
            }
        };

        let checksum_hex = self.0.config.wasm_checksum.as_ref().ok_or_else(|| {
            eyre::eyre!("wasm_checksum not configured — required for Solana light client creation")
        })?;
        let checksum =
            hex::decode(checksum_hex).map_err(|e| eyre::eyre!("decoding wasm_checksum: {e}"))?;

        let latest_slot = extract_latest_slot(&client_state);

        let wasm_client_state = WasmClientState {
            data: client_state,
            checksum,
            latest_height: Some(ProtoHeight {
                revision_number: 0,
                revision_height: latest_slot,
            }),
        };

        let wasm_consensus_state = WasmConsensusState {
            data: consensus_state,
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
        payload: SolanaUpdateClientPayload,
    ) -> Result<UpdateClientOutput<CosmosMessage>> {
        let signer = self.0.signer.account_address()?;

        let headers = match payload {
            SolanaUpdateClientPayload::Dummy { headers } => headers,
            SolanaUpdateClientPayload::Attested { proof, .. } => {
                let msg = MsgUpdateClient {
                    client_id: client_id.to_string(),
                    client_message: Some(any(attestations::ATTESTATION_PROOF_TYPE_URL, &proof)),
                    signer,
                };
                return Ok(UpdateClientOutput::messages_only(vec![to_any(&msg)]));
            }
        };

        let messages = headers
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
        counterparty_client_id: &SolanaClientId,
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

fn solana_packet_to_v2(packet: &SolanaPacket) -> V2Packet {
    V2Packet {
        sequence: packet.sequence.into(),
        source_client: packet.source_client_id.clone(),
        destination_client: packet.dest_client_id.clone(),
        timeout_timestamp: packet.timeout_timestamp.into(),
        payloads: packet
            .payloads
            .iter()
            .map(|p| channel::Payload {
                source_port: p.source_port.0.clone(),
                destination_port: p.dest_port.0.clone(),
                version: p.version.clone(),
                encoding: p.encoding.clone(),
                value: p.data.clone(),
            })
            .collect(),
    }
}

fn extract_latest_slot(client_state_bytes: &[u8]) -> u64 {
    client_state_bytes
        .get(..8)
        .and_then(|b| b.try_into().ok())
        .map_or(0, u64::from_le_bytes)
}

/// Placeholder proof sent when `mock_proofs` is enabled; ibc-go rejects empty proofs
/// before the (dummy) light client ever sees them.
const MOCK_PROOF: &[u8] = b"mock";

impl<S: CosmosSigner> CosmosAdapter<S> {
    /// Solana has no state proofs, so a packet is proven either with a mock proof
    /// (dummy light client) or with an attestation fetched here, where the full
    /// packet is known. Returns the proof bytes and proof height for the message.
    async fn solana_proof(
        &self,
        proof: SolanaCommitmentProof,
        packet: &V2Packet,
        commitment_type: CommitmentType,
        proof_height: SolanaHeight,
        revision: u64,
    ) -> Result<(Vec<u8>, ProtoHeight)> {
        if self.0.config.mock_proofs {
            return Ok((
                MOCK_PROOF.to_vec(),
                ProtoHeight {
                    revision_number: 0,
                    revision_height: 0,
                },
            ));
        }
        let SolanaCommitmentProof::Attested(aggregator) = proof else {
            eyre::bail!(
                "Solana packet proofs need attestors; configure `attestation` on the Solana chain \
                 or enable `mock_proofs` for a dummy light client"
            );
        };
        let attestation = aggregator
            .packet_attestation(
                vec![abi_encode_packet(packet)],
                proof_height.0,
                commitment_type,
            )
            .await?;
        Ok((
            attestation.into_proof().encode_to_vec(),
            ProtoHeight {
                revision_number: revision,
                revision_height: proof_height.into(),
            },
        ))
    }
}

fn any(type_url: &str, msg: &impl prost::Message) -> ibc_proto::google::protobuf::Any {
    ibc_proto::google::protobuf::Any {
        type_url: type_url.to_string(),
        value: msg.encode_to_vec(),
    }
}

#[async_trait]
impl<S: CosmosSigner> PacketMessageBuilder<SolanaChain> for CosmosAdapter<S> {
    async fn build_receive_packet_message(
        &self,
        packet: &SolanaPacket,
        proof: SolanaCommitmentProof,
        proof_height: SolanaHeight,
        revision: u64,
    ) -> Result<CosmosMessage> {
        let packet = solana_packet_to_v2(packet);
        let (proof_commitment, proof_height) = self
            .solana_proof(
                proof,
                &packet,
                CommitmentType::Packet,
                proof_height,
                revision,
            )
            .await?;
        let msg = MsgRecvPacket {
            packet: Some(packet),
            proof_commitment,
            proof_height: Some(proof_height),
            signer: self.0.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }

    async fn build_ack_packet_message(
        &self,
        packet: &SolanaPacket,
        ack: &SolanaAcknowledgement,
        proof: SolanaCommitmentProof,
        proof_height: SolanaHeight,
        revision: u64,
    ) -> Result<CosmosMessage> {
        let acknowledgement =
            channel::Acknowledgement::decode(ack.0.as_slice()).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "ack proto decode failed, treating raw bytes as single app-ack");
                channel::Acknowledgement {
                    app_acknowledgements: vec![ack.0.clone()],
                }
            });
        let packet = solana_packet_to_v2(packet);
        let (proof_acked, proof_height) = self
            .solana_proof(proof, &packet, CommitmentType::Ack, proof_height, revision)
            .await?;
        let msg = MsgAcknowledgement {
            packet: Some(packet),
            acknowledgement: Some(acknowledgement),
            proof_acked,
            proof_height: Some(proof_height),
            signer: self.0.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &CosmosPacket,
        proof: SolanaCommitmentProof,
        proof_height: SolanaHeight,
        revision: u64,
    ) -> Result<CosmosMessage> {
        use mercury_cosmos::builders::cosmos_packet_to_v2;
        let packet = cosmos_packet_to_v2(packet);
        let (proof_unreceived, proof_height) = self
            .solana_proof(
                proof,
                &packet,
                CommitmentType::Receipt,
                proof_height,
                revision,
            )
            .await?;
        let msg = MsgTimeout {
            packet: Some(packet),
            proof_unreceived,
            proof_height: Some(proof_height),
            signer: self.0.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourDetector<SolanaChain> for CosmosAdapter<S> {
    type UpdateHeader = mercury_cosmos::misbehaviour::OnChainTmConsensusState;
    type MisbehaviourEvidence = mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence;
    type CounterpartyClientState = SolanaClientState;

    #[tracing::instrument(skip_all, name = "cosmos_check_misbehaviour_for_solana")]
    async fn check_for_misbehaviour(
        &self,
        _client_id: &SolanaClientId,
        on_chain: &mercury_cosmos::misbehaviour::OnChainTmConsensusState,
        _client_state: &SolanaClientState,
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
            "MISBEHAVIOUR DETECTED: consensus state mismatch (Cosmos→Solana)"
        );

        eyre::bail!(
            "misbehaviour detected at height {} (Cosmos→Solana): \
             corrective update submission not yet implemented for Solana",
            on_chain.height
        )
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourQuery<SolanaChain> for CosmosAdapter<S> {
    type CounterpartyUpdateHeader = ibc_client_tendermint::types::Header;

    async fn query_consensus_state_heights(
        &self,
        _client_id: &Self::ClientId,
    ) -> Result<Vec<SolanaHeight>> {
        Ok(vec![])
    }

    async fn query_update_client_header(
        &self,
        _client_id: &Self::ClientId,
        _consensus_height: &SolanaHeight,
    ) -> Result<Option<ibc_client_tendermint::types::Header>> {
        Ok(None)
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourMessageBuilder<SolanaChain> for CosmosAdapter<S> {
    type MisbehaviourEvidence = SolanaMisbehaviourEvidence;

    async fn build_misbehaviour_message(
        &self,
        _client_id: &Self::ClientId,
        _evidence: SolanaMisbehaviourEvidence,
    ) -> Result<CosmosMessage> {
        eyre::bail!("Solana misbehaviour submission not yet supported")
    }
}
