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

const DEFAULT_TRUSTING_PERIOD: Duration = Duration::from_hours(14 * 24);
const DEFAULT_UNBONDING_PERIOD: Duration = Duration::from_hours(21 * 24);
pub(crate) const DEFAULT_MAX_CLOCK_DRIFT: Duration = Duration::from_secs(40);

#[derive(Clone, Debug, Default)]
pub struct CosmosCreateClientPayload {
    pub client_state: Any,
    pub consensus_state: Any,
    pub counterparty_client_id: Option<String>,
    pub counterparty_merkle_prefix: Option<mercury_core::MerklePrefix>,
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

        let (trusted_consensus_state, trusted_next_validator_set) =
            fetch_trusted_state(self, *trusted_height).await?;

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
            trusted_next_validator_set,
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

/// Returns the consensus state at `trusted_height` and the validator set that
/// signs `trusted_height + 1`, which is what the header's `trusted_validators`
/// must hash to (the trusted block's `next_validators_hash`).
pub(crate) async fn fetch_trusted_state<S: CosmosSigner>(
    chain: &CosmosChain<S>,
    trusted_height: TmHeight,
) -> Result<(Option<TendermintConsensusState>, ValidatorSet)> {
    let next_height = trusted_height.increment();
    let (next_validators_response, trusted_commit_response) = chain
        .rpc_guard
        .guarded_pair(
            || async {
                chain
                    .rpc_client
                    .validators(next_height, Paging::All)
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

    let header = trusted_commit_response.signed_header.header;
    // The trusted block's proposer is only a best guess for the next set;
    // proposer does not affect the set hash.
    let proposer = find_proposer(
        &next_validators_response.validators,
        &header.proposer_address,
    );
    let next_validator_set = ValidatorSet::new(next_validators_response.validators, proposer);
    Ok((
        Some(TendermintConsensusState::from(header)),
        next_validator_set,
    ))
}

pub(crate) fn find_proposer(
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
        assert_eq!(v2.payloads, vec![]);
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

        let result = find_proposer(std::slice::from_ref(&validator), &validator.address);
        assert!(result.is_some());
        assert_eq!(result.unwrap().address, validator.address);
    }

    #[test]
    fn default_has_none_counterparty_fields() {
        let p = CosmosCreateClientPayload::default();
        assert!(p.counterparty_client_id.is_none());
        assert!(p.counterparty_merkle_prefix.is_none());
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

    #[test]
    fn find_proposer_empty_set() {
        let addr = tendermint::account::Id::new([1u8; 20]);
        assert!(find_proposer(&[], &addr).is_none());
    }

    mod messages {
        use super::*;
        use crate::chain::test_support::{test_chain, test_signer};
        use crate::keys::Secp256k1KeyPair;
        use ibc::core::host::types::identifiers::ClientId;
        use ibc_proto::ibc::core::client::v1::MsgUpgradeClient;
        use prost::Name;

        type TestChain = CosmosChain<Secp256k1KeyPair>;

        fn signer() -> String {
            test_signer().account_address().unwrap()
        }

        fn client_id() -> ClientId {
            "07-tendermint-0".parse().unwrap()
        }

        fn any(type_url: &str, value: &[u8]) -> Any {
            Any {
                type_url: type_url.into(),
                value: value.to_vec(),
            }
        }

        fn packet() -> CosmosPacket {
            CosmosPacket {
                source_client_id: RawClientId("07-tendermint-1".into()),
                dest_client_id: RawClientId("07-tendermint-0".into()),
                sequence: PacketSequence(11),
                timeout_timestamp: TimeoutTimestamp(99),
                payloads: vec![],
            }
        }

        fn proof() -> MerkleProof {
            MerkleProof {
                proof_bytes: vec![0xaa, 0xbb],
            }
        }

        fn height(h: u64) -> TmHeight {
            TmHeight::try_from(h).unwrap()
        }

        fn decode<M: prost::Message + Name + Default>(msg: &CosmosMessage) -> M {
            assert_eq!(msg.type_url.as_ref(), M::type_url());
            M::decode(msg.value.as_slice()).unwrap()
        }

        #[tokio::test]
        async fn create_client_message_wraps_payload() {
            let chain = test_chain();
            let payload = CosmosCreateClientPayload {
                client_state: any("/cs", b"client"),
                consensus_state: any("/cons", b"consensus"),
                ..Default::default()
            };
            let msg = chain.build_create_client_message(payload).await.unwrap();
            let decoded: MsgCreateClient = decode(&msg);
            assert_eq!(decoded.client_state, Some(any("/cs", b"client")));
            assert_eq!(decoded.consensus_state, Some(any("/cons", b"consensus")));
            assert_eq!(decoded.signer, signer());
        }

        #[tokio::test]
        async fn update_client_message_one_per_header() {
            let chain = test_chain();
            let payload = CosmosUpdateClientPayload {
                headers: vec![any("/h", b"1"), any("/h", b"2")],
                trusted_consensus_state: None,
                membership_proofs: MembershipProofs::new(),
            };
            let output = chain
                .build_update_client_message(&client_id(), payload)
                .await
                .unwrap();
            assert!(output.membership_proof.is_none());
            assert_eq!(output.messages.len(), 2);
            for (msg, expected) in output.messages.iter().zip([b"1", b"2"]) {
                let decoded: MsgUpdateClient = decode(msg);
                assert_eq!(decoded.client_id, "07-tendermint-0");
                assert_eq!(decoded.client_message, Some(any("/h", expected)));
                assert_eq!(decoded.signer, signer());
            }
        }

        #[tokio::test]
        async fn update_client_message_no_headers_no_messages() {
            let chain = test_chain();
            let payload = CosmosUpdateClientPayload {
                headers: vec![],
                trusted_consensus_state: None,
                membership_proofs: MembershipProofs::new(),
            };
            let output = chain
                .build_update_client_message(&client_id(), payload)
                .await
                .unwrap();
            assert!(output.messages.is_empty());
        }

        #[tokio::test]
        async fn register_counterparty_message_fields() {
            let chain = test_chain();
            let counterparty: ClientId = "07-tendermint-9".parse().unwrap();
            let msg = chain
                .build_register_counterparty_message(
                    &client_id(),
                    &counterparty,
                    mercury_core::MerklePrefix::ibc_default(),
                )
                .await
                .unwrap();
            let decoded: MsgRegisterCounterparty = decode(&msg);
            assert_eq!(decoded.client_id, "07-tendermint-0");
            assert_eq!(decoded.counterparty_client_id, "07-tendermint-9");
            assert_eq!(
                decoded.counterparty_merkle_prefix,
                vec![b"ibc".to_vec(), vec![]]
            );
            assert_eq!(decoded.signer, signer());
        }

        fn upgrade_payload(
            client_state: Vec<u8>,
            consensus_state: Vec<u8>,
        ) -> UpgradeClientPayload {
            UpgradeClientPayload {
                plan_height: 100,
                upgraded_client_state: client_state,
                upgraded_consensus_state: consensus_state,
                proof_upgrade_client: vec![1],
                proof_upgrade_consensus_state: vec![2],
            }
        }

        #[tokio::test]
        async fn upgrade_client_message_decodes_upgraded_states() {
            let chain = test_chain();
            let cs = any("/upgraded.ClientState", b"cs");
            let cons = any("/upgraded.ConsensusState", b"cons");
            let msgs = chain
                .build_upgrade_client_message(
                    &client_id(),
                    upgrade_payload(cs.encode_to_vec(), cons.encode_to_vec()),
                )
                .await
                .unwrap();
            assert_eq!(msgs.len(), 1);
            let decoded: MsgUpgradeClient = decode(&msgs[0]);
            assert_eq!(decoded.client_id, "07-tendermint-0");
            assert_eq!(decoded.client_state, Some(cs));
            assert_eq!(decoded.consensus_state, Some(cons));
            assert_eq!(decoded.proof_upgrade_client, vec![1]);
            assert_eq!(decoded.proof_upgrade_consensus_state, vec![2]);
            assert_eq!(decoded.signer, signer());
        }

        #[tokio::test]
        async fn upgrade_client_message_rejects_malformed_client_state() {
            let chain = test_chain();
            let cons = any("/c", b"c").encode_to_vec();
            let err = chain
                .build_upgrade_client_message(
                    &client_id(),
                    upgrade_payload(vec![0xff, 0xff, 0xff], cons),
                )
                .await
                .unwrap_err();
            assert!(
                format!("{err:#}").contains("upgraded client state"),
                "{err:#}"
            );
        }

        #[tokio::test]
        async fn upgrade_client_message_rejects_malformed_consensus_state() {
            let chain = test_chain();
            let cs = any("/c", b"c").encode_to_vec();
            let err = chain
                .build_upgrade_client_message(&client_id(), upgrade_payload(cs, vec![0xff, 0xff]))
                .await
                .unwrap_err();
            assert!(
                format!("{err:#}").contains("upgraded consensus state"),
                "{err:#}"
            );
        }

        #[tokio::test]
        async fn receive_packet_message_fields() {
            let chain = test_chain();
            let msg = chain
                .build_receive_packet_message(&packet(), proof(), height(50), 4)
                .await
                .unwrap();
            let decoded: MsgRecvPacket = decode(&msg);
            assert_eq!(decoded.packet, Some(cosmos_packet_to_v2(&packet())));
            assert_eq!(decoded.proof_commitment, vec![0xaa, 0xbb]);
            assert_eq!(decoded.proof_height, Some(to_proto_height(4, height(50))));
            assert_eq!(decoded.signer, signer());
        }

        #[tokio::test]
        async fn ack_packet_message_decodes_proto_ack() {
            let chain = test_chain();
            let ack = channel::Acknowledgement {
                app_acknowledgements: vec![b"a".to_vec(), b"b".to_vec()],
            };
            let msg = chain
                .build_ack_packet_message(
                    &packet(),
                    &PacketAcknowledgement(ack.encode_to_vec()),
                    proof(),
                    height(7),
                    2,
                )
                .await
                .unwrap();
            let decoded: MsgAcknowledgement = decode(&msg);
            assert_eq!(decoded.acknowledgement, Some(ack));
            assert_eq!(decoded.proof_acked, vec![0xaa, 0xbb]);
            assert_eq!(decoded.proof_height, Some(to_proto_height(2, height(7))));
            assert_eq!(decoded.packet.unwrap().sequence, 11);
        }

        #[tokio::test]
        async fn ack_packet_message_wraps_undecodable_ack_as_single_app_ack() {
            let chain = test_chain();
            let raw = vec![0xff, 0xff, 0xff];
            let msg = chain
                .build_ack_packet_message(
                    &packet(),
                    &PacketAcknowledgement(raw.clone()),
                    proof(),
                    height(7),
                    2,
                )
                .await
                .unwrap();
            let decoded: MsgAcknowledgement = decode(&msg);
            assert_eq!(
                decoded.acknowledgement.unwrap().app_acknowledgements,
                vec![raw]
            );
        }

        #[tokio::test]
        async fn timeout_packet_message_fields() {
            let chain = test_chain();
            let msg = chain
                .build_timeout_packet_message(&packet(), proof(), height(30), 1)
                .await
                .unwrap();
            let decoded: MsgTimeout = decode(&msg);
            assert_eq!(decoded.packet, Some(cosmos_packet_to_v2(&packet())));
            assert_eq!(decoded.proof_unreceived, vec![0xaa, 0xbb]);
            assert_eq!(decoded.proof_height, Some(to_proto_height(1, height(30))));
            assert_eq!(decoded.signer, signer());
        }

        #[tokio::test]
        async fn update_client_payload_rejects_non_increasing_target() {
            let chain = test_chain();
            let placeholder = crate::client_types::CosmosClientState::placeholder();
            for (trusted, target) in [(10, 10), (10, 9)] {
                let err =
                    <TestChain as ClientPayloadBuilder<TestChain>>::build_update_client_payload(
                        &chain,
                        &height(trusted),
                        &height(target),
                        &placeholder,
                    )
                    .await
                    .unwrap_err();
                assert!(err.to_string().contains("must be greater"), "{err}");
            }
        }
    }

    mod mocked {
        use super::*;
        use crate::chain::test_support::grpc::{GrpcReply, reply, spawn_grpc, unimplemented};
        use crate::chain::test_support::rpc::{MockRpc, Reply, RpcCall, spawn_rpc};
        use crate::chain::test_support::tm::{
            abci_reply, block_reply, block_time, commit_reply, header, proof_ops, set_hash,
            status_json, validators, validators_reply,
        };
        use crate::chain::test_support::{
            UNREACHABLE_ADDR, mock_chain, mock_config, test_chain_with, tm_client_state,
        };
        use crate::client_types::CosmosClientState;
        use crate::keys::Secp256k1KeyPair;
        use ibc_proto::cosmos::upgrade::v1beta1::{Plan, QueryCurrentPlanResponse};

        type TestChain = CosmosChain<Secp256k1KeyPair>;

        const PLAN_PATH: &str = "/cosmos.upgrade.v1beta1.Query/CurrentPlan";

        fn height(h: u64) -> TmHeight {
            TmHeight::try_from(h).unwrap()
        }

        fn height_param(call: &RpcCall) -> u64 {
            call.params["height"].as_str().unwrap().parse().unwrap()
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

        #[tokio::test]
        async fn create_client_payload_from_latest_block() {
            let vals = validators(&[1, 2]);
            let next = validators(&[3]);
            let latest = header(50, 0x11, &vals, &next);
            let rpc = spawn_rpc(move |call| match call.method.as_str() {
                "block" => block_reply(latest.clone()),
                other => Reply::Error(format!("unexpected {other}")),
            })
            .await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let payload =
                <TestChain as ClientPayloadBuilder<TestChain>>::build_create_client_payload(&chain)
                    .await
                    .unwrap();

            let cs = TendermintClientState::try_from(payload.client_state).unwrap();
            assert_eq!(cs.chain_id.as_str(), "testchain-1");
            assert_eq!(cs.latest_height, Height::new(1, 50).unwrap());
            assert_eq!(cs.trust_level, TrustThreshold::ONE_THIRD);
            assert_eq!(cs.trusting_period, DEFAULT_TRUSTING_PERIOD);
            assert_eq!(cs.unbonding_period, DEFAULT_UNBONDING_PERIOD);
            assert_eq!(cs.max_clock_drift, DEFAULT_MAX_CLOCK_DRIFT);
            assert_eq!(cs.proof_specs, ProofSpecs::cosmos());
            assert_eq!(cs.upgrade_path, vec!["upgrade", "upgradedIBCState"]);

            let cons = TendermintConsensusState::try_from(payload.consensus_state).unwrap();
            assert_eq!(cons.root.as_bytes(), &[0x11; 32]);
            assert_eq!(cons.timestamp(), block_time(50));
            assert_eq!(cons.next_validators_hash, set_hash(&next));
            assert!(payload.counterparty_client_id.is_none());
            // latest block, not a pinned height
            assert!(
                rpc.calls_to("block")[0]
                    .get("height")
                    .is_none_or(serde_json::Value::is_null)
            );
        }

        #[tokio::test]
        async fn create_client_payload_honours_configured_periods() {
            let latest = header(7, 0, &validators(&[1]), &validators(&[1]));
            let rpc = spawn_rpc(move |_| block_reply(latest.clone())).await;
            let mut config = mock_config(&rpc.url, UNREACHABLE_ADDR);
            config.trusting_period = Some(Duration::from_secs(100));
            config.unbonding_period = Some(Duration::from_secs(200));
            config.max_clock_drift = Some(Duration::from_secs(3));
            let chain = test_chain_with(config);
            let payload =
                <TestChain as ClientPayloadBuilder<TestChain>>::build_create_client_payload(&chain)
                    .await
                    .unwrap();
            let cs = TendermintClientState::try_from(payload.client_state).unwrap();
            assert_eq!(cs.trusting_period, Duration::from_secs(100));
            assert_eq!(cs.unbonding_period, Duration::from_secs(200));
            assert_eq!(cs.max_clock_drift, Duration::from_secs(3));
        }

        #[tokio::test]
        async fn create_client_payload_rpc_error_propagates() {
            let rpc = spawn_rpc(|_| Reply::Error("no block".into())).await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            assert!(
                <TestChain as ClientPayloadBuilder<TestChain>>::build_create_client_payload(&chain)
                    .await
                    .is_err()
            );
        }

        async fn header_node() -> MockRpc {
            spawn_rpc(|call| {
                let h = height_param(call);
                let vals = validators(&[1, 2, 3]);
                match call.method.as_str() {
                    "validators" => validators_reply(h, &vals),
                    "commit" => commit_reply(header(h, u8::try_from(h).unwrap(), &vals, &vals)),
                    other => Reply::Error(format!("unexpected {other}")),
                }
            })
            .await
        }

        #[tokio::test]
        async fn update_client_payload_fetches_trusted_and_target_headers() {
            let rpc = header_node().await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let cs = CosmosClientState::Tendermint(tm_client_state(9));
            let payload =
                <TestChain as ClientPayloadBuilder<TestChain>>::build_update_client_payload(
                    &chain,
                    &height(9),
                    &height(15),
                    &cs,
                )
                .await
                .unwrap();

            // skip verification: only the target header, trusting height 9
            assert_eq!(payload.headers.len(), 1);
            let header = TmIbcHeader::try_from(payload.headers[0].clone()).unwrap();
            assert_eq!(header.signed_header.header.height.value(), 15);
            assert_eq!(header.signed_header.header.app_hash.as_bytes(), &[15u8; 32]);
            assert_eq!(
                header.validator_set.hash(),
                header.signed_header.header.validators_hash
            );
            assert_eq!(
                header.validator_set.proposer().as_ref().map(|p| p.address),
                Some(header.signed_header.header.proposer_address)
            );
            assert_eq!(header.trusted_height, Height::new(1, 9).unwrap());

            let trusted = payload.trusted_consensus_state.unwrap();
            assert_eq!(trusted.root.as_bytes(), &[9u8; 32]);
            assert_eq!(
                header.trusted_next_validator_set.hash(),
                trusted.next_validators_hash
            );

            assert_eq!(heights_requested(&rpc, "validators"), vec![10, 15]);
            assert_eq!(heights_requested(&rpc, "commit"), vec![9, 15]);
        }

        // Validator set {1,2,3} up to height 10; validator 4 joins at 11. Header
        // hashes are consistent and the target commit carries real signatures.
        const OLD_SET: &[u8] = &[1, 2, 3];
        const NEW_SET: &[u8] = &[1, 2, 3, 4];
        const LAST_OLD_HEIGHT: u64 = 10;

        fn set_at(h: u64) -> &'static [u8] {
            if h <= LAST_OLD_HEIGHT {
                OLD_SET
            } else {
                NEW_SET
            }
        }

        fn chain_header(h: u64) -> tendermint::block::Header {
            header(
                h,
                u8::try_from(h).unwrap(),
                &validators(set_at(h)),
                &validators(set_at(h + 1)),
            )
        }

        async fn valset_change_node() -> MockRpc {
            use tendermint_rpc::endpoint::commit;
            spawn_rpc(|call| {
                let h = height_param(call);
                match call.method.as_str() {
                    "validators" => validators_reply(h, &validators(set_at(h))),
                    "commit" => crate::chain::test_support::rpc::ok(&commit::Response {
                        signed_header: crate::chain::test_support::tm::signed_header_by(
                            chain_header(h),
                            set_at(h),
                            10,
                        ),
                        canonical: true,
                    }),
                    other => Reply::Error(format!("unexpected {other}")),
                }
            })
            .await
        }

        // Mirrors ibc-client-tendermint 0.57 `client_state::verify_header`
        // (the function the SP1 ICS07 program and ibc-rs hosts run): validate_basic,
        // trusted next-validator-set hash check, then the light client verifier.
        fn ibc_rs_verify(
            header: &TmIbcHeader,
            trusted: &TendermintConsensusState,
        ) -> std::result::Result<(), String> {
            use tendermint_light_client_verifier::Verifier as _;
            use tendermint_light_client_verifier::options::Options;
            use tendermint_light_client_verifier::types::{
                TrustThreshold as TmTrust, TrustedBlockState, UntrustedBlockState,
            };
            type H = tendermint::crypto::default::Sha256;

            header.validate_basic::<H>().map_err(|e| e.to_string())?;
            header
                .check_trusted_next_validator_set::<H>(&trusted.next_validators_hash)
                .map_err(|e| e.to_string())?;
            let chain_id: tendermint::chain::Id = "testchain-1".parse().unwrap();
            let trusted_state = TrustedBlockState {
                chain_id: &chain_id,
                header_time: trusted.timestamp(),
                height: TmHeight::try_from(header.trusted_height.revision_height()).unwrap(),
                next_validators: &header.trusted_next_validator_set,
                next_validators_hash: trusted.next_validators_hash,
            };
            let untrusted = UntrustedBlockState {
                signed_header: &header.signed_header,
                validators: &header.validator_set,
                next_validators: None,
            };
            let options = Options {
                trust_threshold: TmTrust::ONE_THIRD,
                trusting_period: Duration::from_secs(3600),
                clock_drift: Duration::from_secs(10),
            };
            let now = block_time(100);
            match tendermint_light_client_verifier::ProdVerifier::default().verify_update_header(
                untrusted,
                trusted_state,
                &options,
                now,
            ) {
                tendermint_light_client_verifier::Verdict::Success => Ok(()),
                other => Err(format!("{other:?}")),
            }
        }

        #[tokio::test]
        async fn update_client_header_verifies_across_validator_set_change() {
            let rpc = valset_change_node().await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let cs = CosmosClientState::Tendermint(tm_client_state(LAST_OLD_HEIGHT));
            let payload =
                <TestChain as ClientPayloadBuilder<TestChain>>::build_update_client_payload(
                    &chain,
                    &height(LAST_OLD_HEIGHT),
                    &height(20),
                    &cs,
                )
                .await
                .unwrap();
            let header = TmIbcHeader::try_from(payload.headers[0].clone()).unwrap();
            let trusted = payload.trusted_consensus_state.unwrap();

            // Control: the same header with the trusted set taken from H+1 verifies,
            // so the fixture (hashes, signatures, times) is sound.
            let mut fixed = header.clone();
            fixed.trusted_next_validator_set =
                tendermint::validator::Set::new(validators(set_at(LAST_OLD_HEIGHT + 1)), None);
            ibc_rs_verify(&fixed, &trusted).expect("H+1 trusted set must verify");

            ibc_rs_verify(&header, &trusted).expect("header built by mercury must verify");
        }

        #[tokio::test]
        async fn corrective_update_header_verifies_across_validator_set_change() {
            let rpc = valset_change_node().await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let payload = crate::misbehaviour::build_corrective_update_payload(
                &chain,
                height(LAST_OLD_HEIGHT),
                height(20),
            )
            .await
            .unwrap();
            let header = TmIbcHeader::try_from(payload.headers[0].clone()).unwrap();
            let trusted = payload.trusted_consensus_state.unwrap();
            ibc_rs_verify(&header, &trusted).expect("corrective header must verify");
        }

        #[tokio::test]
        async fn update_client_payload_rpc_error_propagates() {
            let rpc = spawn_rpc(|_| Reply::Error("pruned".into())).await;
            let chain = mock_chain(&rpc.url, UNREACHABLE_ADDR);
            let cs = CosmosClientState::Tendermint(tm_client_state(9));
            assert!(
                <TestChain as ClientPayloadBuilder<TestChain>>::build_update_client_payload(
                    &chain,
                    &height(9),
                    &height(15),
                    &cs,
                )
                .await
                .is_err()
            );
        }

        fn plan_reply(plan_height: Option<i64>) -> GrpcReply {
            reply(&QueryCurrentPlanResponse {
                plan: plan_height.map(|height| Plan {
                    name: "v2".into(),
                    height,
                    info: String::new(),
                    ..Default::default()
                }),
            })
        }

        // x/upgrade UpgradedClientKey / UpgradedConsStateKey
        fn upgraded_key(plan_height: u64, leaf: &str) -> String {
            hex::encode_upper(format!("upgradedIBCState/{plan_height}/{leaf}"))
        }

        async fn upgrade_payload(
            plan: Option<i64>,
            latest: u64,
            client_value: Vec<u8>,
        ) -> (Result<Option<UpgradeClientPayload>>, MockRpc) {
            let grpc = spawn_grpc(move |call| match call.path.as_str() {
                PLAN_PATH => plan_reply(plan),
                _ => unimplemented(call),
            })
            .await;
            let rpc = spawn_rpc(move |call| match call.method.as_str() {
                "status" => Reply::Result(status_json(latest, false, true)),
                "abci_query" if call.params["data"] == upgraded_key(100, "upgradedClient") => {
                    abci_reply(client_value.clone(), Some(proof_ops(&[b"client"])))
                }
                "abci_query" => abci_reply(vec![0xc5], Some(proof_ops(&[b"cons", b"root"]))),
                other => Reply::Error(format!("unexpected {other}")),
            })
            .await;
            let chain = mock_chain(&rpc.url, &grpc.url);
            let result =
                <TestChain as ClientPayloadBuilder<TestChain>>::build_upgrade_client_payload(
                    &chain,
                )
                .await;
            (result, rpc)
        }

        #[tokio::test]
        async fn upgrade_payload_reads_upgraded_states_at_latest_height() {
            let (result, rpc) = upgrade_payload(Some(100), 120, vec![0xc1]).await;
            let payload = result.unwrap().unwrap();
            assert_eq!(payload.plan_height, 100);
            assert_eq!(payload.upgraded_client_state, vec![0xc1]);
            assert_eq!(payload.upgraded_consensus_state, vec![0xc5]);
            let decode = |bytes: &[u8]| {
                ibc_proto::ibc::core::commitment::v1::MerkleProof::decode(bytes)
                    .unwrap()
                    .proofs
                    .len()
            };
            assert_eq!(decode(&payload.proof_upgrade_client), 1);
            assert_eq!(decode(&payload.proof_upgrade_consensus_state), 2);

            let queries = rpc.calls_to("abci_query");
            assert_eq!(queries.len(), 2);
            for (q, leaf) in queries.iter().zip(["upgradedClient", "upgradedConsState"]) {
                assert_eq!(q["path"], "store/upgrade/key");
                assert_eq!(q["data"], upgraded_key(100, leaf));
                assert_eq!(q["height"], "120");
                assert_eq!(q["prove"], true);
            }
        }

        #[tokio::test]
        async fn upgrade_payload_none_without_reached_plan_or_state() {
            let (result, rpc) = upgrade_payload(None, 120, vec![1]).await;
            assert!(result.unwrap().is_none());
            assert!(rpc.calls().is_empty(), "no plan, no RPC");

            let (result, rpc) = upgrade_payload(Some(0), 120, vec![1]).await;
            assert!(result.unwrap().is_none());
            assert!(rpc.calls().is_empty());

            let (result, rpc) = upgrade_payload(Some(100), 99, vec![1]).await;
            assert!(result.unwrap().is_none());
            assert!(
                rpc.calls_to("abci_query").is_empty(),
                "plan height not reached"
            );

            let (result, rpc) = upgrade_payload(Some(100), 100, vec![]).await;
            assert!(result.unwrap().is_none());
            assert_eq!(
                rpc.calls_to("abci_query").len(),
                1,
                "no upgraded client stored"
            );
        }

        #[tokio::test]
        async fn upgrade_payload_plan_query_error_propagates() {
            let grpc = spawn_grpc(|_| GrpcReply::Err(tonic::Status::unavailable("down"))).await;
            let chain = mock_chain(UNREACHABLE_ADDR, &grpc.url);
            assert!(
                <TestChain as ClientPayloadBuilder<TestChain>>::build_upgrade_client_payload(
                    &chain
                )
                .await
                .is_err()
            );
        }

        #[tokio::test]
        async fn upgrade_payload_without_proof_is_error() {
            let grpc = spawn_grpc(|_| plan_reply(Some(5))).await;
            let rpc = spawn_rpc(|call| match call.method.as_str() {
                "status" => Reply::Result(status_json(6, false, true)),
                _ => abci_reply(vec![1], None),
            })
            .await;
            let chain = mock_chain(&rpc.url, &grpc.url);
            assert!(
                <TestChain as ClientPayloadBuilder<TestChain>>::build_upgrade_client_payload(
                    &chain
                )
                .await
                .is_err()
            );
        }
    }
}
