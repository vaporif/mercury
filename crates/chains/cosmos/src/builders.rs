use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, StreamExt, TryStreamExt};
use ibc::core::client::types::Height;
use ibc::core::commitment_types::specs::ProofSpecs;
use ibc_client_tendermint::types::{
    AllowUpdate, ClientState as TendermintClientState, ConsensusState as TendermintConsensusState,
    Header as TmIbcHeader, TrustThreshold,
};
use ibc_proto::google::protobuf::Any;
use ibc_proto::ibc::core::client::v1::{Height as ProtoHeight, MsgCreateClient, MsgUpdateClient};
use prost::Message as _;
use tendermint::account;
use tendermint::block::Height as TmHeight;
use tendermint::validator::{Info as ValidatorInfo, Set as ValidatorSet};
use tendermint_rpc::{Client, Paging};
use tracing::instrument;

use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, PacketMessageBuilder,
};
use mercury_chain_traits::types::IbcTypes;
use mercury_core::error::Result;

use ibc_proto::ibc::core::channel::v2::{
    self as channel, MsgAcknowledgement, MsgRecvPacket, MsgTimeout, Packet as V2Packet,
};
use ibc_proto::ibc::core::client::v2::MsgRegisterCounterparty;

use crate::chain::CosmosChain;
use crate::keys::CosmosSigner;
use crate::types::to_any;
use crate::types::{CosmosMessage, CosmosPacket, MerkleProof, PacketAcknowledgement};

const DEFAULT_TRUSTING_PERIOD: Duration = Duration::from_secs(14 * 24 * 3600);
const DEFAULT_UNBONDING_PERIOD: Duration = Duration::from_secs(21 * 24 * 3600);
const DEFAULT_MAX_CLOCK_DRIFT: Duration = Duration::from_secs(40);
const HEADER_FETCH_CONCURRENCY: usize = 8;

/// Payload for creating a Tendermint light client on a counterparty chain.
#[derive(Clone, Debug)]
pub struct CosmosCreateClientPayload {
    pub client_state: Any,
    pub consensus_state: Any,
}

/// Payload containing headers to update a Tendermint light client.
#[derive(Clone, Debug)]
pub struct CosmosUpdateClientPayload {
    pub headers: Vec<Any>,
}

/// Proof data needed to build packet relay messages.
#[derive(Clone, Debug)]
pub struct CosmosProofPayload {
    pub proof: MerkleProof,
    pub proof_height: TmHeight,
    pub proof_revision_number: u64,
}

impl From<(MerkleProof, TmHeight, u64)> for CosmosProofPayload {
    fn from((proof, proof_height, proof_revision_number): (MerkleProof, TmHeight, u64)) -> Self {
        Self {
            proof,
            proof_height,
            proof_revision_number,
        }
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientPayloadBuilder<Self> for CosmosChain<S> {
    type CreateClientPayload = CosmosCreateClientPayload;
    type UpdateClientPayload = CosmosUpdateClientPayload;

    #[instrument(skip_all, name = "build_create_client_payload")]
    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload> {
        let latest_block = self.rpc_client.latest_block().await?;

        let latest_height = latest_block.block.header.height;

        let ibc_height = Height::new(self.chain_id.revision_number(), latest_height.value())
            .map_err(|e| eyre::eyre!("{e}"))?;

        let trusting_period = self
            .config
            .trusting_period
            .unwrap_or(DEFAULT_TRUSTING_PERIOD);
        let unbonding_period = self
            .config
            .unbonding_period
            .unwrap_or(DEFAULT_UNBONDING_PERIOD);
        let max_clock_drift = self
            .config
            .max_clock_drift
            .unwrap_or(DEFAULT_MAX_CLOCK_DRIFT);

        let client_state = TendermintClientState::new(
            self.chain_id.clone(),
            TrustThreshold::ONE_THIRD,
            trusting_period,
            unbonding_period,
            max_clock_drift,
            ibc_height,
            ProofSpecs::cosmos(),
            vec!["upgrade".to_string(), "upgradedIBCState".to_string()],
            AllowUpdate {
                after_expiry: true,
                after_misbehaviour: true,
            },
        )
        .map_err(|e| eyre::eyre!("{e}"))?;

        let consensus_state = TendermintConsensusState::from(latest_block.block.header);

        Ok(CosmosCreateClientPayload {
            client_state: client_state.into(),
            consensus_state: consensus_state.into(),
        })
    }

    #[instrument(skip_all, name = "build_update_client_payload", fields(trusted = %trusted_height, target = %target_height))]
    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
        _counterparty_client_state: &<Self as IbcTypes<Self>>::ClientState,
    ) -> Result<Self::UpdateClientPayload> {
        let trusted_height_value = trusted_height.value();
        let target_height_value = target_height.value();

        if target_height_value <= trusted_height_value {
            eyre::bail!(
                "target height ({target_height_value}) must be greater than trusted height ({trusted_height_value})"
            );
        }

        let (trusted_validators_response, trusted_commit_response) = tokio::try_join!(
            async {
                self.rpc_client
                    .validators(*trusted_height, Paging::All)
                    .await
                    .map_err(eyre::Report::from)
            },
            async {
                self.rpc_client
                    .commit(*trusted_height)
                    .await
                    .map_err(eyre::Report::from)
            },
        )?;

        let trusted_proposer = find_proposer(
            &trusted_validators_response.validators,
            &trusted_commit_response
                .signed_header
                .header
                .proposer_address,
        );
        let trusted_next_validator_set =
            ValidatorSet::new(trusted_validators_response.validators, trusted_proposer);

        let ibc_trusted_height = Height::new(self.chain_id.revision_number(), trusted_height_value)
            .map_err(|e| eyre::eyre!("{e}"))?;

        let heights: Vec<u64> = ((trusted_height_value + 1)..=target_height_value).collect();

        let headers: Vec<Any> = stream::iter(heights)
            .map(|h| {
                let rpc = &self.rpc_client;
                let trusted_vs = &trusted_next_validator_set;
                async move {
                    let height = TmHeight::try_from(h)?;

                    let (commit_response, validators_response) = tokio::try_join!(
                        async { rpc.commit(height).await.map_err(eyre::Report::from) },
                        async {
                            rpc.validators(height, Paging::All)
                                .await
                                .map_err(eyre::Report::from)
                        },
                    )?;

                    let proposer = find_proposer(
                        &validators_response.validators,
                        &commit_response.signed_header.header.proposer_address,
                    );
                    let validator_set = ValidatorSet::new(validators_response.validators, proposer);

                    let header = TmIbcHeader {
                        signed_header: commit_response.signed_header,
                        validator_set,
                        trusted_height: ibc_trusted_height,
                        trusted_next_validator_set: trusted_vs.clone(),
                    };

                    Ok::<_, eyre::Report>(header.into())
                }
            })
            .buffered(HEADER_FETCH_CONCURRENCY)
            .try_collect()
            .await?;

        Ok(CosmosUpdateClientPayload { headers })
    }
}

fn find_proposer(
    validators: &[ValidatorInfo],
    proposer_address: &account::Id,
) -> Option<ValidatorInfo> {
    validators
        .iter()
        .find(|v| &v.address == proposer_address)
        .cloned()
}

#[async_trait]
impl<S: CosmosSigner> ClientMessageBuilder<Self> for CosmosChain<S> {
    async fn build_create_client_message(
        &self,
        payload: CosmosCreateClientPayload,
    ) -> Result<CosmosMessage> {
        let signer = self.signer.account_address()?;

        let msg = MsgCreateClient {
            client_state: Some(payload.client_state),
            consensus_state: Some(payload.consensus_state),
            signer,
        };

        Ok(to_any(&msg))
    }

    async fn build_update_client_message(
        &self,
        client_id: &Self::ClientId,
        payload: CosmosUpdateClientPayload,
    ) -> Result<Vec<CosmosMessage>> {
        let signer = self.signer.account_address()?;

        let messages = payload
            .headers
            .into_iter()
            .map(|header_any| {
                let msg = MsgUpdateClient {
                    client_id: client_id.to_string(),
                    client_message: Some(header_any),
                    signer: signer.clone(),
                };
                to_any(&msg)
            })
            .collect();

        Ok(messages)
    }

    async fn build_register_counterparty_message(
        &self,
        client_id: &Self::ClientId,
        counterparty_client_id: &Self::ClientId,
        counterparty_merkle_prefix: mercury_core::MerklePrefix,
    ) -> Result<CosmosMessage> {
        let signer = self.signer.account_address()?;

        let msg = MsgRegisterCounterparty {
            client_id: client_id.to_string(),
            counterparty_merkle_prefix: counterparty_merkle_prefix.0,
            counterparty_client_id: counterparty_client_id.to_string(),
            signer,
        };

        Ok(to_any(&msg))
    }
}

fn cosmos_packet_to_v2(packet: &CosmosPacket) -> V2Packet {
    V2Packet {
        sequence: packet.sequence,
        source_client: packet.source_client_id.to_string(),
        destination_client: packet.dest_client_id.to_string(),
        timeout_timestamp: packet.timeout_timestamp,
        payloads: packet
            .payloads
            .iter()
            .map(|p| channel::Payload {
                source_port: p.source_port.clone(),
                destination_port: p.dest_port.clone(),
                version: p.version.clone(),
                encoding: p.encoding.clone(),
                value: p.data.clone(),
            })
            .collect(),
    }
}

fn to_proto_height(revision_number: u64, h: TmHeight) -> ProtoHeight {
    ProtoHeight {
        revision_number,
        revision_height: h.value(),
    }
}

#[async_trait]
impl<S: CosmosSigner> PacketMessageBuilder<Self> for CosmosChain<S> {
    type ReceivePacketPayload = CosmosProofPayload;
    type AckPacketPayload = CosmosProofPayload;
    type TimeoutPacketPayload = CosmosProofPayload;

    async fn build_receive_packet_message(
        &self,
        packet: &CosmosPacket,
        payload: Self::ReceivePacketPayload,
    ) -> Result<CosmosMessage> {
        let msg = MsgRecvPacket {
            packet: Some(cosmos_packet_to_v2(packet)),
            proof_commitment: payload.proof.proof_bytes,
            proof_height: Some(to_proto_height(
                payload.proof_revision_number,
                payload.proof_height,
            )),
            signer: self.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }

    async fn build_ack_packet_message(
        &self,
        packet: &CosmosPacket,
        ack: &PacketAcknowledgement,
        payload: Self::AckPacketPayload,
    ) -> Result<CosmosMessage> {
        // ack.0 stores full proto-encoded Acknowledgement bytes from write_ack event
        let acknowledgement =
            channel::Acknowledgement::decode(ack.0.as_slice()).unwrap_or_else(|_| {
                channel::Acknowledgement {
                    app_acknowledgements: vec![ack.0.clone()],
                }
            });
        let msg = MsgAcknowledgement {
            packet: Some(cosmos_packet_to_v2(packet)),
            acknowledgement: Some(acknowledgement),
            proof_acked: payload.proof.proof_bytes,
            proof_height: Some(to_proto_height(
                payload.proof_revision_number,
                payload.proof_height,
            )),
            signer: self.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &CosmosPacket,
        payload: Self::TimeoutPacketPayload,
    ) -> Result<CosmosMessage> {
        let msg = MsgTimeout {
            packet: Some(cosmos_packet_to_v2(packet)),
            proof_unreceived: payload.proof.proof_bytes,
            proof_height: Some(to_proto_height(
                payload.proof_revision_number,
                payload.proof_height,
            )),
            signer: self.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibc::core::host::types::identifiers::ClientId;

    use crate::types::PacketPayload;

    #[test]
    fn cosmos_packet_to_v2_converts_fields() {
        let packet = CosmosPacket {
            source_client_id: ClientId::new("07-tendermint", 0).unwrap(),
            dest_client_id: ClientId::new("07-tendermint", 1).unwrap(),
            sequence: 5,
            timeout_timestamp: 12345,
            payloads: vec![PacketPayload {
                source_port: "transfer".to_string(),
                dest_port: "transfer".to_string(),
                version: "ics20-1".to_string(),
                encoding: "json".to_string(),
                data: b"test".to_vec(),
            }],
        };

        let v2 = cosmos_packet_to_v2(&packet);
        assert_eq!(v2.sequence, 5);
        assert_eq!(v2.source_client, "07-tendermint-0");
        assert_eq!(v2.destination_client, "07-tendermint-1");
        assert_eq!(v2.timeout_timestamp, 12345);
        assert_eq!(v2.payloads.len(), 1);
        assert_eq!(v2.payloads[0].source_port, "transfer");
        assert_eq!(v2.payloads[0].destination_port, "transfer");
        assert_eq!(v2.payloads[0].value, b"test");
    }

    #[test]
    fn cosmos_packet_to_v2_empty_payloads() {
        let packet = CosmosPacket {
            source_client_id: ClientId::new("07-tendermint", 0).unwrap(),
            dest_client_id: ClientId::new("07-tendermint", 1).unwrap(),
            sequence: 1,
            timeout_timestamp: 0,
            payloads: vec![],
        };
        let v2 = cosmos_packet_to_v2(&packet);
        assert!(v2.payloads.is_empty());
    }

    #[test]
    fn to_proto_height_converts() {
        let h = TmHeight::try_from(100u64).unwrap();
        let proto = to_proto_height(3, h);
        assert_eq!(proto.revision_number, 3);
        assert_eq!(proto.revision_height, 100);
    }

    #[test]
    fn find_proposer_found() {
        use tendermint::public_key::PublicKey;
        use tendermint::validator::Info as ValidatorInfo;

        let key_bytes = [1u8; 32];
        let pk = PublicKey::from_raw_ed25519(&key_bytes).unwrap();
        let validator = ValidatorInfo {
            address: tendermint::account::Id::from(pk),
            pub_key: pk,
            power: 10u32.into(),
            name: None,
            proposer_priority: 0.into(),
        };

        let result = find_proposer(&[validator.clone()], &validator.address);
        assert!(result.is_some());
        assert_eq!(result.unwrap().address, validator.address);
    }

    #[test]
    fn find_proposer_not_found() {
        let key_bytes = [1u8; 32];
        let pk = tendermint::public_key::PublicKey::from_raw_ed25519(&key_bytes).unwrap();
        let validator = tendermint::validator::Info {
            address: tendermint::account::Id::from(pk),
            pub_key: pk,
            power: 10u32.into(),
            name: None,
            proposer_priority: 0.into(),
        };

        let other_key = [2u8; 32];
        let other_pk = tendermint::public_key::PublicKey::from_raw_ed25519(&other_key).unwrap();
        let other_addr = tendermint::account::Id::from(other_pk);

        let result = find_proposer(&[validator], &other_addr);
        assert!(result.is_none());
    }
}
