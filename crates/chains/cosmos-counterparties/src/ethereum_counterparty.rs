//! Cross-chain trait impls for `CosmosChain<S>` with `EthereumChainInner` counterparty.
//!
//! Implements: `ClientQuery`, `ClientMessageBuilder`, `PacketMessageBuilder`,
//! `MisbehaviourDetector`, `MisbehaviourQuery`, `MisbehaviourMessageBuilder`.
//! Gated behind the `ethereum-beacon` feature.

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

use mercury_ethereum::builders::encode_evm_proof;
use mercury_ethereum::builders::{CreateClientPayload, UpdateClientPayload};
use mercury_ethereum::chain::EthereumChainInner;
use mercury_ethereum::types::{
    EvmAcknowledgement, EvmClientState, EvmCommitmentProof, EvmHeight, EvmPacket,
};

use ethereum_light_client::client_state::ClientState as EthClientState;
use ibc_proto::ibc::core::channel::v2::{
    self as channel, MsgAcknowledgement, MsgRecvPacket, MsgTimeout, Packet as V2Packet,
};
use ibc_proto::ibc::core::client::v1::{Height as ProtoHeight, MsgCreateClient, MsgUpdateClient};
use ibc_proto::ibc::core::client::v2::MsgRegisterCounterparty;
use ibc_proto::ibc::lightclients::wasm::v1::{
    ClientState as WasmClientState, ConsensusState as WasmConsensusState,
};
use prost::Message as _;

use crate::wrapper::CosmosChain;

#[async_trait]
impl<S: CosmosSigner> ClientQuery<EthereumChainInner> for CosmosChain<S> {
    async fn query_client_state(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Self::ClientState> {
        // Query cosmos gRPC for the WASM-wrapped Beacon light client state.
        // Delegates to the same-chain query since the on-chain storage is identical.
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
        /// Beacon sync committee period is ~27 hours.
        const BEACON_TRUSTING_PERIOD: Duration = Duration::from_secs(24 * 3600);

        match client_state {
            CosmosClientState::Wasm(_) => Some(BEACON_TRUSTING_PERIOD),
            CosmosClientState::Tendermint(_) => {
                tracing::warn!("unexpected Tendermint client state for Ethereum counterparty");
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
            CosmosClientState::Tendermint(_) => {
                tracing::warn!("unexpected Tendermint client state for Ethereum counterparty");
                EvmHeight(0)
            }
        }
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientMessageBuilder<EthereumChainInner> for CosmosChain<S> {
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
                let msg = MsgUpdateClient {
                    client_id: client_id.to_string(),
                    client_message: Some(ibc_proto::google::protobuf::Any {
                        type_url: "/ibc.lightclients.wasm.v1.ClientMessage".to_string(),
                        value: header_bytes,
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
        counterparty_client_id: &<EthereumChainInner as mercury_chain_traits::types::ChainTypes>::ClientId,
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
        sequence: packet.sequence,
        source_client: packet.source_client.clone(),
        destination_client: packet.dest_client.clone(),
        timeout_timestamp: packet.timeout_timestamp,
        payloads: packet
            .payloads
            .iter()
            .map(|p| channel::Payload {
                source_port: p.source_port.clone(),
                destination_port: p.dest_port.clone(),
                version: p.version.clone(),
                encoding: p.encoding.clone(),
                value: p.value.clone(),
            })
            .collect(),
    }
}

#[async_trait]
impl<S: CosmosSigner> PacketMessageBuilder<EthereumChainInner> for CosmosChain<S> {
    async fn build_receive_packet_message(
        &self,
        packet: &EvmPacket,
        proof: EvmCommitmentProof,
        proof_height: EvmHeight,
        revision: u64,
    ) -> Result<CosmosMessage> {
        let msg = MsgRecvPacket {
            packet: Some(evm_packet_to_v2(packet)),
            proof_commitment: encode_evm_proof(&proof),
            proof_height: Some(ProtoHeight {
                revision_number: revision,
                revision_height: proof_height.0,
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
            proof_acked: encode_evm_proof(&proof),
            proof_height: Some(ProtoHeight {
                revision_number: revision,
                revision_height: proof_height.0,
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
        let msg = MsgTimeout {
            packet: Some(cosmos_packet_to_v2(packet)),
            proof_unreceived: encode_evm_proof(&proof),
            proof_height: Some(ProtoHeight {
                revision_number: revision,
                revision_height: proof_height.0,
            }),
            signer: self.0.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }
}

// -- Misbehaviour: CosmosChain as Src, EthereumChain as Dst --
// Cosmos detects Tendermint misbehaviour for headers submitted to Ethereum's SP1 light client.
// Same detection logic as Cosmos→Cosmos — compares submitted header against on-chain commit.

#[async_trait]
impl<S: CosmosSigner> MisbehaviourDetector<EthereumChainInner> for CosmosChain<S> {
    type UpdateHeader = ibc_client_tendermint::types::Header;
    type MisbehaviourEvidence = mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence;
    type CounterpartyClientState = EvmClientState;

    #[tracing::instrument(skip_all, name = "cosmos_check_misbehaviour_for_eth")]
    async fn check_for_misbehaviour(
        &self,
        client_id: &<EthereumChainInner as mercury_chain_traits::types::ChainTypes>::ClientId,
        update_header: &ibc_client_tendermint::types::Header,
        _client_state: &EvmClientState,
    ) -> Result<Option<mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence>> {
        use ibc_client_tendermint::types::Misbehaviour as TmMisbehaviour;
        use tendermint::validator::Set as ValidatorSet;
        use tendermint_rpc::{Client, Paging};

        let header_height = update_header.signed_header.header.height;

        let (commit_response, validators_response) = tokio::try_join!(
            async {
                self.0
                    .rpc_client
                    .commit(header_height)
                    .await
                    .map_err(eyre::Report::from)
            },
            async {
                self.0
                    .rpc_client
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
            eth_client = %client_id,
            "MISBEHAVIOUR DETECTED: conflicting Tendermint headers (Cosmos→Ethereum)"
        );

        let proposer = validators_response
            .validators
            .iter()
            .find(|v| v.address == commit_response.signed_header.header.proposer_address)
            .cloned();
        let validator_set = ValidatorSet::new(validators_response.validators, proposer);

        let challenging_header = ibc_client_tendermint::types::Header {
            signed_header: commit_response.signed_header,
            validator_set,
            trusted_height: update_header.trusted_height,
            trusted_next_validator_set: update_header.trusted_next_validator_set.clone(),
        };

        let ibc_client_id: ibc::core::host::types::identifiers::ClientId = client_id
            .0
            .parse()
            .map_err(|e| eyre::eyre!("invalid client ID for misbehaviour: {e}"))?;

        let misbehaviour =
            TmMisbehaviour::new(ibc_client_id, update_header.clone(), challenging_header);

        Ok(Some(
            mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence {
                misbehaviour,
                supporting_headers: Vec::new(),
            },
        ))
    }
}

// -- Misbehaviour: CosmosChain as Dst, EthereumChain as Src (beacon chain) --
// Cosmos can't yet detect beacon chain misbehaviour or query Ethereum headers from its wasm LC.

#[async_trait]
impl<S: CosmosSigner> MisbehaviourQuery<EthereumChainInner> for CosmosChain<S> {
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
impl<S: CosmosSigner> MisbehaviourMessageBuilder<EthereumChainInner> for CosmosChain<S> {
    type MisbehaviourEvidence = ();

    async fn build_misbehaviour_message(
        &self,
        _client_id: &Self::ClientId,
        _evidence: (),
    ) -> Result<CosmosMessage> {
        eyre::bail!("beacon chain misbehaviour not yet supported")
    }
}
