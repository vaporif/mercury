use std::collections::HashMap;
use std::time::Duration;

use alloy::consensus::Transaction as _;
use alloy::primitives::U256;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use eyre::Context;
use tendermint::block::Height as TmHeight;
use tracing::instrument;

use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder, UpdateClientOutput,
};
use mercury_chain_traits::queries::{ClientQuery, MisbehaviourQuery};
use mercury_chain_traits::types::ChainTypes;
use mercury_core::MerklePrefix;
use mercury_core::error::Result;

use mercury_cosmos::builders::{CosmosCreateClientPayload, CosmosUpdateClientPayload};
use mercury_cosmos::chain::CosmosChain;
use mercury_cosmos::keys::CosmosSigner;
use mercury_cosmos::types::{CosmosPacket, MerkleProof, PacketAcknowledgement};

use crate::wrapper::EthereumAdapter;
use mercury_cosmos::client_types::CosmosClientState;
use mercury_ethereum::builders::{
    CreateClientPayload as EvmCreateClientPayload, UpdateClientPayload as EvmUpdateClientPayload,
};
use mercury_ethereum::chain::to_sol_merkle_prefix;
use mercury_ethereum::contracts::{
    ICS26Router, IICS02ClientMsgs, IICS26RouterMsgs, SP1ICS07Tendermint,
};
use mercury_ethereum::queries::{decode_client_state, encode_client_state, resolve_light_client};
use mercury_ethereum::types::{
    EvmClientId, EvmClientState, EvmConsensusState, EvmHeight, EvmMessage, EvmPacket,
};

#[async_trait]
impl<S: CosmosSigner> ClientPayloadBuilder<CosmosChain<S>> for EthereumAdapter {
    type CreateClientPayload = EvmCreateClientPayload;
    type UpdateClientPayload = EvmUpdateClientPayload;

    async fn build_create_client_payload(&self) -> Result<EvmCreateClientPayload> {
        self.0.build_create_client_payload().await
    }

    async fn build_update_client_payload(
        &self,
        trusted_height: &EvmHeight,
        target_height: &EvmHeight,
        counterparty_client_state: &<CosmosChain<S> as mercury_chain_traits::types::IbcTypes>::ClientState,
    ) -> Result<EvmUpdateClientPayload> {
        // The counterparty (Cosmos) stores the Ethereum beacon light client
        // state inside a WASM envelope. Extract the inner `data` bytes, which
        // are the JSON-serialized `ethereum_light_client::ClientState`.
        let inner_bytes = match counterparty_client_state {
            CosmosClientState::Wasm(wasm_cs) => &wasm_cs.data,
            CosmosClientState::Tendermint(_) => {
                eyre::bail!(
                    "expected Wasm-wrapped beacon client state on Cosmos counterparty, got Tendermint"
                );
            }
        };

        let evm_client_state = EvmClientState(inner_bytes.clone());
        self.0
            .build_update_client_payload(trusted_height, target_height, &evm_client_state)
            .await
    }

    fn update_payload_proof_height(&self, payload: &EvmUpdateClientPayload) -> Option<EvmHeight> {
        self.0.update_payload_proof_height(payload)
    }

    fn update_payload_message_height(&self, payload: &EvmUpdateClientPayload) -> Option<EvmHeight> {
        self.0.update_payload_message_height(payload)
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientQuery<CosmosChain<S>> for EthereumAdapter {
    #[instrument(skip_all, name = "query_client_state", fields(chain = %self.chain_label(), client_id = %client_id))]
    async fn query_client_state(
        &self,
        client_id: &EvmClientId,
        _height: &EvmHeight,
    ) -> Result<EvmClientState> {
        let lc_address = resolve_light_client(&self.0, client_id).await?;
        let lc = SP1ICS07Tendermint::new(lc_address, &*self.provider);
        // Use clientState() directly (struct accessor) instead of getClientState()
        // (bytes wrapper) to avoid ABI decode mismatches.
        let cs = self
            .0
            .rpc_guard
            .guarded(|| async {
                lc.clientState()
                    .call()
                    .await
                    .wrap_err("SP1ICS07Tendermint.clientState() failed")
            })
            .await?;
        tracing::debug!(
            chain_id = %cs.chainId,
            revision_height = cs.latestHeight.revisionHeight,
            "queried SP1 client state (cross-chain)"
        );
        Ok(EvmClientState(encode_client_state(&cs)))
    }

    #[instrument(skip_all, name = "query_consensus_state", fields(chain = %self.chain_label(), client_id = %client_id, consensus_height = %consensus_height))]
    async fn query_consensus_state(
        &self,
        client_id: &EvmClientId,
        consensus_height: &TmHeight,
        _query_height: &EvmHeight,
    ) -> Result<EvmConsensusState> {
        let height_u64 = consensus_height.value();
        let lc_address = resolve_light_client(&self.0, client_id).await?;
        let lc = SP1ICS07Tendermint::new(lc_address, &*self.provider);
        let result = self
            .0
            .rpc_guard
            .guarded(|| async {
                lc.getConsensusStateHash(height_u64)
                    .call()
                    .await
                    .wrap_err_with(|| format!("getConsensusStateHash({height_u64}) failed"))
            })
            .await?;
        Ok(EvmConsensusState(result.to_vec()))
    }

    fn trusting_period(client_state: &EvmClientState) -> Option<Duration> {
        let cs = decode_client_state(&client_state.0)?;
        Some(Duration::from_secs(u64::from(cs.trustingPeriod)))
    }

    fn client_latest_height(client_state: &EvmClientState) -> TmHeight {
        decode_client_state(&client_state.0).map_or_else(
            || {
                tracing::warn!("failed to decode client state, defaulting to height 1");
                TmHeight::from(1_u32)
            },
            |cs| {
                TmHeight::try_from(cs.latestHeight.revisionHeight)
                    .unwrap_or_else(|_| TmHeight::from(1_u32))
            },
        )
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientMessageBuilder<CosmosChain<S>> for EthereumAdapter {
    type CreateClientPayload = CosmosCreateClientPayload;
    type UpdateClientPayload = CosmosUpdateClientPayload;

    async fn build_create_client_message(
        &self,
        _payload: CosmosCreateClientPayload,
    ) -> Result<EvmMessage> {
        // The SP1ICS07Tendermint light client contract is deployed separately
        // with its initial state. We just register it on the ICS26Router.
        // Counterparty info is set later via `build_register_counterparty_message`.
        let call = ICS26Router::addClient_1Call {
            counterpartyInfo: IICS02ClientMsgs::CounterpartyInfo {
                clientId: String::new(),
                merklePrefix: Vec::new(),
            },
            client: self.config.light_client_address()?,
        };

        Ok(EvmMessage {
            to: self.router_address,
            calldata: call.abi_encode(),
            value: U256::ZERO,
        })
    }

    async fn build_update_client_message(
        &self,
        client_id: &EvmClientId,
        payload: CosmosUpdateClientPayload,
    ) -> Result<UpdateClientOutput<EvmMessage>> {
        let sp1 = self
            .sp1
            .as_ref()
            .ok_or_else(|| eyre::eyre!("SP1 prover not configured"))?;

        let headers: Vec<Vec<u8>> = payload
            .headers
            .iter()
            .map(prost::Message::encode_to_vec)
            .collect();

        self.0
            .build_update_client_message_sp1(
                client_id,
                headers,
                payload.trusted_consensus_state,
                payload.membership_proofs,
                sp1,
            )
            .await
    }

    async fn build_register_counterparty_message(
        &self,
        client_id: &EvmClientId,
        counterparty_client_id: &<CosmosChain<S> as mercury_chain_traits::types::ChainTypes>::ClientId,
        counterparty_merkle_prefix: MerklePrefix,
    ) -> Result<EvmMessage> {
        let call = ICS26Router::migrateClientCall {
            clientId: client_id.0.clone(),
            counterpartyInfo: IICS02ClientMsgs::CounterpartyInfo {
                clientId: counterparty_client_id.to_string(),
                merklePrefix: to_sol_merkle_prefix(&counterparty_merkle_prefix),
            },
            client: self.config.light_client_address()?,
        };

        Ok(EvmMessage {
            to: self.router_address,
            calldata: call.abi_encode(),
            value: U256::ZERO,
        })
    }

    fn enrich_update_payload(
        &self,
        payload: &mut CosmosUpdateClientPayload,
        proofs: &[mercury_core::MembershipProofEntry],
    ) {
        for entry in proofs {
            payload.membership_proofs.push(entry.clone());
        }
    }

    fn finalize_batch(
        &self,
        update_output: &mut UpdateClientOutput<EvmMessage>,
        packet_messages: &mut [EvmMessage],
    ) {
        let Some(proof_bytes) = update_output.membership_proof.take() else {
            return;
        };

        // Inject the combined membership proof into the first packet message's proof field.
        let Some(first_msg) = packet_messages.first_mut() else {
            tracing::warn!(
                "combined membership proof generated but no packet messages — proof discarded"
            );
            return;
        };

        inject_membership_proof(first_msg, &proof_bytes);
    }
}

// TODO: refactor — use a macro to collapse the repetitive decode/inject/encode branches
// or forget about it...
fn inject_membership_proof(msg: &mut EvmMessage, proof_bytes: &[u8]) {
    if let Ok(mut call) = ICS26Router::recvPacketCall::abi_decode(&msg.calldata) {
        call.msg_.proofCommitment = proof_bytes.to_vec().into();
        msg.calldata = call.abi_encode();
    } else if let Ok(mut call) = ICS26Router::ackPacketCall::abi_decode(&msg.calldata) {
        call.msg_.proofAcked = proof_bytes.to_vec().into();
        msg.calldata = call.abi_encode();
    } else if let Ok(mut call) = ICS26Router::timeoutPacketCall::abi_decode(&msg.calldata) {
        call.msg_.proofTimeout = proof_bytes.to_vec().into();
        msg.calldata = call.abi_encode();
    } else {
        tracing::warn!(
            "finalize_batch: first packet message has unrecognized calldata, skipping proof injection"
        );
    }
}

fn cosmos_packet_to_sol(packet: &CosmosPacket) -> IICS26RouterMsgs::Packet {
    IICS26RouterMsgs::Packet {
        sequence: packet.sequence.into(),
        sourceClient: packet.source_client_id.0.clone(),
        destClient: packet.dest_client_id.0.clone(),
        timeoutTimestamp: packet.timeout_timestamp.into(),
        payloads: packet
            .payloads
            .iter()
            .map(|p| IICS26RouterMsgs::Payload {
                sourcePort: p.source_port.clone().into(),
                destPort: p.dest_port.clone().into(),
                version: p.version.clone(),
                encoding: p.encoding.clone(),
                value: p.data.clone().into(),
            })
            .collect(),
    }
}

/// In the SP1 batched proving model, membership proofs are bundled into the
/// preceding `updateClient` call via `enrich_update_payload`. The packet
/// messages themselves carry empty proof bytes — the on-chain verifier checks
/// them against the already-verified state root.
#[async_trait]
impl<S: CosmosSigner> PacketMessageBuilder<CosmosChain<S>> for EthereumAdapter {
    async fn build_receive_packet_message(
        &self,
        packet: &CosmosPacket,
        _proof: MerkleProof,
        proof_height: TmHeight,
        revision: u64,
    ) -> Result<EvmMessage> {
        let call = ICS26Router::recvPacketCall {
            msg_: IICS26RouterMsgs::MsgRecvPacket {
                packet: cosmos_packet_to_sol(packet),
                proofCommitment: Vec::new().into(),
                proofHeight: IICS02ClientMsgs::Height {
                    revisionNumber: revision,
                    revisionHeight: proof_height.value(),
                },
            },
        };

        Ok(EvmMessage {
            to: self.router_address,
            calldata: call.abi_encode(),
            value: U256::ZERO,
        })
    }

    async fn build_ack_packet_message(
        &self,
        packet: &CosmosPacket,
        ack: &PacketAcknowledgement,
        _proof: MerkleProof,
        proof_height: TmHeight,
        revision: u64,
    ) -> Result<EvmMessage> {
        let call = ICS26Router::ackPacketCall {
            msg_: IICS26RouterMsgs::MsgAckPacket {
                packet: cosmos_packet_to_sol(packet),
                acknowledgement: ack.0.clone().into(),
                proofAcked: Vec::new().into(),
                proofHeight: IICS02ClientMsgs::Height {
                    revisionNumber: revision,
                    revisionHeight: proof_height.value(),
                },
            },
        };

        Ok(EvmMessage {
            to: self.router_address,
            calldata: call.abi_encode(),
            value: U256::ZERO,
        })
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &EvmPacket,
        _proof: MerkleProof,
        proof_height: TmHeight,
        revision: u64,
    ) -> Result<EvmMessage> {
        let sol_packet = IICS26RouterMsgs::Packet {
            sequence: packet.sequence.into(),
            sourceClient: packet.source_client.clone(),
            destClient: packet.dest_client.clone(),
            timeoutTimestamp: packet.timeout_timestamp.into(),
            payloads: packet
                .payloads
                .iter()
                .map(|p| IICS26RouterMsgs::Payload {
                    sourcePort: p.source_port.clone().into(),
                    destPort: p.dest_port.clone().into(),
                    version: p.version.clone(),
                    encoding: p.encoding.clone(),
                    value: p.value.clone().into(),
                })
                .collect(),
        };

        let call = ICS26Router::timeoutPacketCall {
            msg_: IICS26RouterMsgs::MsgTimeoutPacket {
                packet: sol_packet,
                proofTimeout: Vec::new().into(),
                proofHeight: IICS02ClientMsgs::Height {
                    revisionNumber: revision,
                    revisionHeight: proof_height.value(),
                },
            },
        };

        Ok(EvmMessage {
            to: self.router_address,
            calldata: call.abi_encode(),
            value: U256::ZERO,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mercury_chain_traits::builders::ClientPayloadBuilder;
    use mercury_chain_traits::types::IbcTypes;

    /// Compile-time verification that cross-chain trait bounds are satisfied.
    fn _assert_cross_chain_traits<S: CosmosSigner>()
    where
        EthereumAdapter: IbcTypes
            + ClientQuery<CosmosChain<S>>
            + ClientMessageBuilder<CosmosChain<S>>
            + ClientPayloadBuilder<CosmosChain<S>>,
        CosmosChain<S>: ClientPayloadBuilder<EthereumAdapter>,
    {
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourDetector<CosmosChain<S>> for EthereumAdapter {
    type UpdateHeader = ();
    type MisbehaviourEvidence = ();
    type CounterpartyClientState = mercury_cosmos::client_types::CosmosClientState;

    async fn check_for_misbehaviour(
        &self,
        _client_id: &<CosmosChain<S> as mercury_chain_traits::types::ChainTypes>::ClientId,
        _update_header: &(),
        _client_state: &mercury_cosmos::client_types::CosmosClientState,
    ) -> Result<Option<()>> {
        Ok(None)
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourQuery<CosmosChain<S>> for EthereumAdapter {
    type CounterpartyUpdateHeader = mercury_cosmos::misbehaviour::OnChainTmConsensusState;

    #[instrument(skip_all, name = "eth_query_consensus_heights", fields(chain = %self.chain_label(), client_id = %client_id))]
    async fn query_consensus_state_heights(
        &self,
        client_id: &Self::ClientId,
    ) -> Result<Vec<TmHeight>> {
        use alloy::providers::Provider;
        use alloy::rpc::types::Filter;
        use alloy::sol_types::SolEvent;
        use futures::stream::{self, StreamExt, TryStreamExt};
        use ibc_eureka_solidity_types::msgs::IUpdateClientMsgs;

        let filter = Filter::new()
            .address(self.router_address)
            .event_signature(ICS26Router::ICS02ClientUpdated::SIGNATURE_HASH)
            .from_block(self.config.deployment_block);

        let logs = self
            .0
            .rpc_guard
            .guarded(|| async {
                self.provider
                    .get_logs(&filter)
                    .await
                    .wrap_err("querying ICS02ClientUpdated events")
            })
            .await?;

        let tx_hashes: Vec<_> = logs
            .iter()
            .filter_map(|log| {
                let decoded = ICS26Router::ICS02ClientUpdated::decode_log(log.as_ref()).ok()?;
                if decoded.data.clientId != client_id.0 {
                    return None;
                }
                log.transaction_hash
            })
            .collect();

        // TODO: use JSON-RPC batch requests once alloy exposes `new_batch()` through the Provider trait.
        let txs: Vec<_> = stream::iter(tx_hashes)
            .map(|tx_hash| async move {
                self.provider
                    .get_transaction_by_hash(tx_hash)
                    .await
                    .map_err(eyre::Report::from)
            })
            .buffer_unordered(16)
            .try_collect()
            .await
            .wrap_err("fetching updateClient transactions")?;

        let mut heights = Vec::new();
        let mut new_cache = HashMap::new();

        for tx in txs.into_iter().flatten() {
            let Ok(call) = ICS26Router::updateClientCall::abi_decode(tx.inner.input()) else {
                continue;
            };
            let Ok(msg) =
                <IUpdateClientMsgs::MsgUpdateClient as alloy::sol_types::SolType>::abi_decode(
                    &call.updateMsg,
                )
            else {
                continue;
            };
            let Ok(output) =
                <IUpdateClientMsgs::UpdateClientOutput as alloy::sol_types::SolType>::abi_decode(
                    &msg.sp1Proof.publicValues,
                )
            else {
                continue;
            };

            let Ok(h) = TmHeight::try_from(output.newHeight.revisionHeight) else {
                continue;
            };

            let cs = mercury_cosmos::misbehaviour::OnChainTmConsensusState {
                height: h,
                timestamp_nanos: output.newConsensusState.timestamp,
                root: output.newConsensusState.root.into(),
                next_validators_hash: output.newConsensusState.nextValidatorsHash.into(),
            };
            new_cache.insert(h.value(), cs);
            heights.push(h);
        }

        *self.1.lock().unwrap() = new_cache;

        heights.sort_unstable();
        heights.dedup();
        heights.reverse();

        tracing::debug!(count = heights.len(), "found consensus state heights");
        Ok(heights)
    }

    async fn query_update_client_header(
        &self,
        _client_id: &Self::ClientId,
        consensus_height: &TmHeight,
    ) -> Result<Option<mercury_cosmos::misbehaviour::OnChainTmConsensusState>> {
        let cache = self.1.lock().unwrap();
        Ok(cache.get(&consensus_height.value()).cloned())
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourMessageBuilder<CosmosChain<S>> for EthereumAdapter {
    type MisbehaviourEvidence = mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence;

    #[instrument(skip_all, name = "eth_build_misbehaviour_msg", fields(chain = %self.chain_label(), client_id = %client_id))]
    async fn build_misbehaviour_message(
        &self,
        client_id: &Self::ClientId,
        evidence: mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence,
    ) -> Result<EvmMessage> {
        match evidence {
            mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence::CorrectiveUpdate {
                payload,
            } => self.build_corrective_update_message(client_id, payload).await,
            mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence::Misbehaviour {
                misbehaviour,
                ..
            } => self.build_sp1_misbehaviour_message(client_id, misbehaviour).await,
        }
    }
}

impl EthereumAdapter {
    async fn build_corrective_update_message(
        &self,
        client_id: &EvmClientId,
        payload: mercury_cosmos::builders::CosmosUpdateClientPayload,
    ) -> Result<EvmMessage> {
        tracing::info!("building corrective updateClient to trigger on-chain conflict detection");

        let headers: Vec<Vec<u8>> = payload
            .headers
            .iter()
            .map(prost::Message::encode_to_vec)
            .collect();

        let sp1 = self.sp1.as_ref().ok_or_else(|| {
            eyre::eyre!("SP1 prover not configured — required for corrective update proof")
        })?;

        let output = self
            .0
            .build_update_client_message_sp1(
                client_id,
                headers,
                payload.trusted_consensus_state,
                payload.membership_proofs,
                sp1,
            )
            .await?;

        output
            .messages
            .into_iter()
            .next()
            .ok_or_else(|| eyre::eyre!("SP1 corrective update produced no messages"))
    }

    async fn build_sp1_misbehaviour_message(
        &self,
        client_id: &EvmClientId,
        misbehaviour: ibc_client_tendermint::types::Misbehaviour,
    ) -> Result<EvmMessage> {
        use alloy::sol_types::SolValue;
        use ibc_eureka_solidity_types::msgs::IICS07TendermintMsgs::{
            ClientState as SolClientState, ConsensusState as SolConsensusState,
        };
        use ibc_eureka_solidity_types::msgs::{
            IMisbehaviourMsgs::MsgSubmitMisbehaviour, ISP1Msgs::SP1Proof,
        };
        use ibc_eureka_solidity_types::sp1_ics07::sp1_ics07_tendermint;
        use ibc_proto::Protobuf;
        use mercury_ethereum::queries::resolve_light_client;
        use sp1_prover::types::HashableKey;

        type RawMisbehaviour = ibc_client_tendermint::types::proto::v1::Misbehaviour;

        let sp1 = self.sp1.as_ref().ok_or_else(|| {
            eyre::eyre!("SP1 prover not configured — required for misbehaviour proof")
        })?;

        let lc_address = resolve_light_client(&self.0, client_id).await?;
        let lc = sp1_ics07_tendermint::new(lc_address, &*self.provider);
        let client_state: SolClientState = self
            .0
            .rpc_guard
            .guarded(|| async {
                lc.clientState()
                    .call()
                    .await
                    .wrap_err("SP1ICS07Tendermint.clientState() failed")
            })
            .await?
            .into();

        let trusted_height_1 = misbehaviour.header1().trusted_height.revision_height();
        let trusted_height_2 = misbehaviour.header2().trusted_height.revision_height();

        let h1 = misbehaviour.header1();
        let trusted_cs_1 = SolConsensusState::from(
            ibc_client_tendermint::types::ConsensusState::from(h1.signed_header.header.clone()),
        );

        let h2 = misbehaviour.header2();
        let trusted_cs_2 = if trusted_height_1 == trusted_height_2 {
            trusted_cs_1.clone()
        } else {
            SolConsensusState::from(ibc_client_tendermint::types::ConsensusState::from(
                h2.signed_header.header.clone(),
            ))
        };

        let proto_bytes = <ibc_client_tendermint::types::Misbehaviour as Protobuf<
            RawMisbehaviour,
        >>::encode_vec(misbehaviour);
        let eureka_misbehaviour: ibc_proto_eureka::ibc::lightclients::tendermint::v1::Misbehaviour =
            prost::Message::decode(proto_bytes.as_slice())
                .wrap_err("re-encoding misbehaviour for SP1 prover")?;

        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .wrap_err("system time before UNIX epoch")?
            .as_nanos();

        tracing::info!(
            trusted_height_1,
            trusted_height_2,
            "generating SP1 misbehaviour proof"
        );

        let proof = mercury_ethereum::sp1::generate_misbehaviour_proof_with_timeout(
            sp1,
            client_state,
            eureka_misbehaviour,
            trusted_cs_1,
            trusted_cs_2,
            time,
        )
        .await?;

        tracing::info!("SP1 misbehaviour proof generated");

        let vkey_hex = sp1.misbehaviour_vkey.bytes32();
        let submit_msg = MsgSubmitMisbehaviour {
            sp1Proof: SP1Proof::new(&vkey_hex, proof.bytes(), proof.public_values.to_vec()),
        };

        let call = SP1ICS07Tendermint::misbehaviourCall {
            misbehaviourMsg: submit_msg.abi_encode().into(),
        };

        Ok(EvmMessage {
            to: lc_address,
            calldata: alloy::sol_types::SolCall::abi_encode(&call),
            value: alloy::primitives::U256::ZERO,
        })
    }
}
