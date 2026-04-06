use std::time::Duration;

use async_trait::async_trait;

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
    ClientMessageBuilder, ClientPayloadBuilder, PacketMessageBuilder, UpdateClientOutput,
    UpgradeClientPayload,
};
use mercury_chain_traits::types::{ChainTypes, IbcTypes};
use mercury_core::MembershipProofs;
use mercury_core::error::{Context as _, Result};

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
pub(crate) const DEFAULT_MAX_CLOCK_DRIFT: Duration = Duration::from_secs(40);

/// Payload for creating a Tendermint light client on a counterparty chain.
///
/// The `counterparty_*` / `solana_*` fields carry destination-specific data that
/// certain targets (Solana) need at `add_client` time. Cosmos's own payload
/// builder leaves them `None`; callers populate them after
/// `build_create_client_payload` returns.
#[derive(Clone, Debug, Default)]
pub struct CosmosCreateClientPayload {
    pub client_state: Any,
    pub consensus_state: Any,
    pub counterparty_client_id: Option<String>,
    pub counterparty_merkle_prefix: Option<mercury_core::MerklePrefix>,
    pub solana_client_id: Option<String>,
}

/// Payload containing headers to update a Tendermint light client.
#[derive(Clone, Debug)]
pub struct CosmosUpdateClientPayload {
    pub headers: Vec<Any>,
    pub trusted_consensus_state: Option<TendermintConsensusState>,
    /// Membership proof entries for batched proving.
    /// Populated by `enrich_update_payload` when packet proofs need to be
    /// bundled into the preceding `updateClient` call.
    pub membership_proofs: MembershipProofs,
}

#[async_trait]
impl<S: CosmosSigner, C: ChainTypes> ClientPayloadBuilder<C> for CosmosChain<S> {
    type CreateClientPayload = CosmosCreateClientPayload;
    type UpdateClientPayload = CosmosUpdateClientPayload;

    #[instrument(skip_all, name = "build_create_client_payload", fields(chain = %self.chain_label()))]
    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload> {
        let latest_block = self
            .rpc_guard
            .guarded(|| async { self.rpc_client.latest_block().await.map_err(Into::into) })
            .await?;

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
            ..Default::default()
        })
    }

    #[instrument(skip_all, name = "build_update_client_payload", fields(chain = %self.chain_label(), trusted = %trusted_height, target = %target_height))]
    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
        _counterparty_client_state: &<C as IbcTypes>::ClientState,
    ) -> Result<Self::UpdateClientPayload>
    where
        C: IbcTypes,
    {
        let trusted_height_value = trusted_height.value();
        let target_height_value = target_height.value();

        tracing::debug!(
            trusted_height = trusted_height_value,
            target_height = target_height_value,
            "build_update_client_payload: querying Cosmos RPC"
        );

        if target_height_value <= trusted_height_value {
            eyre::bail!(
                "target height ({target_height_value}) must be greater than trusted height ({trusted_height_value})"
            );
        }

        let (trusted_validators_response, trusted_commit_response) = self
            .rpc_guard
            .guarded_pair(
                || async {
                    self.rpc_client
                        .validators(*trusted_height, Paging::All)
                        .await
                        .map_err(eyre::Report::from)
                },
                || async {
                    self.rpc_client
                        .commit(*trusted_height)
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

        let ibc_trusted_height = Height::new(self.chain_id.revision_number(), trusted_height_value)
            .map_err(|e| eyre::eyre!("{e}"))?;

        // Only fetch the target header — the SP1 light client supports skip verification
        // (verifying a header directly from a non-adjacent trusted height via >1/3 validator overlap).
        let target_tm_height = TmHeight::try_from(target_height_value)?;
        let (commit_response, validators_response) = self
            .rpc_guard
            .guarded_pair(
                || async {
                    self.rpc_client
                        .commit(target_tm_height)
                        .await
                        .map_err(eyre::Report::from)
                },
                || async {
                    self.rpc_client
                        .validators(target_tm_height, Paging::All)
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
            trusted_next_validator_set: trusted_next_validator_set.clone(),
        };

        let headers: Vec<Any> = vec![header.into()];

        Ok(CosmosUpdateClientPayload {
            headers,
            trusted_consensus_state,
            membership_proofs: MembershipProofs::new(),
        })
    }

    #[instrument(skip_all, name = "build_upgrade_client_payload", fields(chain = %self.chain_label()))]
    async fn build_upgrade_client_payload(&self) -> Result<Option<UpgradeClientPayload>> {
        use ibc_proto::cosmos::upgrade::v1beta1::{
            QueryCurrentPlanRequest, query_client::QueryClient as UpgradeQueryClient,
        };

        let plan_response = self
            .rpc_guard
            .guarded(|| async {
                UpgradeQueryClient::new(self.grpc_channel.clone())
                    .current_plan(QueryCurrentPlanRequest {})
                    .await
                    .map(tonic::Response::into_inner)
                    .map_err(Into::into)
            })
            .await?;

        let plan = match plan_response.plan {
            Some(plan) if plan.height > 0 => plan,
            _ => return Ok(None),
        };

        let latest_status = self
            .rpc_guard
            .guarded(|| async { self.rpc_client.status().await.map_err(Into::into) })
            .await?;
        let latest_height = latest_status.sync_info.latest_block_height.value();

        if latest_height < plan.height.cast_unsigned() {
            return Ok(None);
        }

        tracing::info!(plan_height = plan.height, "source chain upgrade detected");

        let proof_height = Some(TmHeight::try_from(latest_height).context("invalid proof height")?);

        let client_key = format!("upgradedIBCState/{}/upgradedClient", plan.height);
        let client_resp = crate::queries::query_abci(
            &self.rpc_client,
            &self.rpc_guard,
            "store/upgrade/key",
            client_key.into_bytes(),
            proof_height,
            true,
        )
        .await?;

        if client_resp.value.is_empty() {
            return Ok(None);
        }

        let cons_key = format!("upgradedIBCState/{}/upgradedConsState", plan.height);
        let cons_resp = crate::queries::query_abci(
            &self.rpc_client,
            &self.rpc_guard,
            "store/upgrade/key",
            cons_key.into_bytes(),
            proof_height,
            true,
        )
        .await?;

        let proof_client = crate::queries::extract_proof(&client_resp)?.proof_bytes;
        let proof_consensus = crate::queries::extract_proof(&cons_resp)?.proof_bytes;

        Ok(Some(UpgradeClientPayload {
            plan_height: plan.height,
            upgraded_client_state: client_resp.value,
            upgraded_consensus_state: cons_resp.value,
            proof_upgrade_client: proof_client,
            proof_upgrade_consensus_state: proof_consensus,
        }))
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
    type CreateClientPayload = CosmosCreateClientPayload;
    type UpdateClientPayload = CosmosUpdateClientPayload;

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
    ) -> Result<UpdateClientOutput<CosmosMessage>> {
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

        Ok(UpdateClientOutput::messages_only(messages))
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

    #[instrument(skip_all, name = "build_upgrade_client_message", fields(chain = %self.chain_label(), client_id = %client_id))]
    async fn build_upgrade_client_message(
        &self,
        client_id: &Self::ClientId,
        payload: UpgradeClientPayload,
    ) -> Result<Vec<CosmosMessage>> {
        use ibc_proto::ibc::core::client::v1::MsgUpgradeClient;

        let client_state = prost::Message::decode(payload.upgraded_client_state.as_slice())
            .context("failed to decode upgraded client state")?;

        let consensus_state = prost::Message::decode(payload.upgraded_consensus_state.as_slice())
            .context("failed to decode upgraded consensus state")?;

        let msg = MsgUpgradeClient {
            client_id: client_id.to_string(),
            client_state: Some(client_state),
            consensus_state: Some(consensus_state),
            proof_upgrade_client: payload.proof_upgrade_client,
            proof_upgrade_consensus_state: payload.proof_upgrade_consensus_state,
            signer: self.signer.account_address()?,
        };

        Ok(vec![to_any(&msg)])
    }
}

#[must_use]
pub fn cosmos_packet_to_v2(packet: &CosmosPacket) -> V2Packet {
    V2Packet {
        sequence: packet.sequence.into(),
        source_client: packet.source_client_id.0.clone(),
        destination_client: packet.dest_client_id.0.clone(),
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

fn to_proto_height(revision_number: u64, h: TmHeight) -> ProtoHeight {
    ProtoHeight {
        revision_number,
        revision_height: h.value(),
    }
}

#[async_trait]
impl<S: CosmosSigner> PacketMessageBuilder<Self> for CosmosChain<S> {
    async fn build_receive_packet_message(
        &self,
        packet: &CosmosPacket,
        proof: MerkleProof,
        proof_height: TmHeight,
        revision: u64,
    ) -> Result<CosmosMessage> {
        let msg = MsgRecvPacket {
            packet: Some(cosmos_packet_to_v2(packet)),
            proof_commitment: proof.proof_bytes,
            proof_height: Some(to_proto_height(revision, proof_height)),
            signer: self.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }

    async fn build_ack_packet_message(
        &self,
        packet: &CosmosPacket,
        ack: &PacketAcknowledgement,
        proof: MerkleProof,
        proof_height: TmHeight,
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
            packet: Some(cosmos_packet_to_v2(packet)),
            acknowledgement: Some(acknowledgement),
            proof_acked: proof.proof_bytes,
            proof_height: Some(to_proto_height(revision, proof_height)),
            signer: self.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &CosmosPacket,
        proof: MerkleProof,
        proof_height: TmHeight,
        revision: u64,
    ) -> Result<CosmosMessage> {
        let msg = MsgTimeout {
            packet: Some(cosmos_packet_to_v2(packet)),
            proof_unreceived: proof.proof_bytes,
            proof_height: Some(to_proto_height(revision, proof_height)),
            signer: self.signer.account_address()?,
        };
        Ok(to_any(&msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PacketPayload, RawClientId};
    use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};

    #[test]
    fn cosmos_packet_to_v2_converts_fields() {
        let packet = CosmosPacket {
            source_client_id: RawClientId("07-tendermint-0".into()),
            dest_client_id: RawClientId("07-tendermint-1".into()),
            sequence: PacketSequence(5),
            timeout_timestamp: TimeoutTimestamp(12345),
            payloads: vec![PacketPayload {
                source_port: Port("transfer".to_string()),
                dest_port: Port("transfer".to_string()),
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
            source_client_id: RawClientId("07-tendermint-0".into()),
            dest_client_id: RawClientId("07-tendermint-1".into()),
            sequence: PacketSequence(1),
            timeout_timestamp: TimeoutTimestamp(0),
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
    fn default_has_none_counterparty_fields() {
        let p = CosmosCreateClientPayload::default();
        assert!(p.counterparty_client_id.is_none());
        assert!(p.counterparty_merkle_prefix.is_none());
        assert!(p.solana_client_id.is_none());
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
