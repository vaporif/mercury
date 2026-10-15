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
            CosmosClientState::Attestations(_) => {
                eyre::bail!(
                    "expected Wasm-wrapped beacon client state on Cosmos counterparty, got Attestations"
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

        *self
            .1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = new_cache;

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
        let cache = self
            .1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            } => {
                self.build_corrective_update_message(client_id, payload)
                    .await
            }
            mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence::Misbehaviour {
                misbehaviour,
                ..
            } => {
                self.build_sp1_misbehaviour_message(client_id, misbehaviour)
                    .await
            }
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

    use alloy::primitives::{Address, B256, Bytes, TxKind};
    use alloy::sol_types::{SolEvent, SolType};
    use ibc_eureka_solidity_types::msgs::{
        IICS02ClientMsgs as SolIcs02, IICS07TendermintMsgs, ISP1Msgs, IUpdateClientMsgs,
    };
    use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};
    use mercury_cosmos::keys::Secp256k1KeyPair;
    use mercury_cosmos::types::{PacketPayload, RawClientId};
    use mercury_ethereum::contracts::sp1_ics07;
    use mercury_ethereum::queries::ClientStateReturn;
    use mercury_ethereum::types::EvmPayload;
    use serde_json::{Value, json};

    use crate::test_support::{LIGHT_CLIENT, ROUTER, adapter, call_target, no_rpc};

    type Cosmos = CosmosChain<Secp256k1KeyPair>;
    type CosmosClientId = <Cosmos as ChainTypes>::ClientId;

    fn sp1_state(height: u64, trusting_period: u32) -> EvmClientState {
        EvmClientState(encode_client_state(&ClientStateReturn {
            chainId: "cosmoshub-4".to_string(),
            trustLevel: sp1_ics07::IICS07TendermintMsgs::TrustThreshold {
                numerator: 1,
                denominator: 3,
            },
            latestHeight: sp1_ics07::IICS02ClientMsgs::Height {
                revisionNumber: 4,
                revisionHeight: height,
            },
            trustingPeriod: trusting_period,
            unbondingPeriod: trusting_period,
            isFrozen: false,
            zkAlgorithm: 0,
        }))
    }

    fn cosmos_packet() -> CosmosPacket {
        CosmosPacket {
            source_client_id: RawClientId("07-tendermint-0".to_string()),
            dest_client_id: RawClientId("client-0".to_string()),
            sequence: PacketSequence(3),
            timeout_timestamp: TimeoutTimestamp(1_900_000_000),
            payloads: vec![PacketPayload {
                source_port: Port("transfer".to_string()),
                dest_port: Port("transfer".to_string()),
                version: "ics20-1".to_string(),
                encoding: "application/json".to_string(),
                data: vec![0xaa],
            }],
        }
    }

    fn evm_packet() -> EvmPacket {
        EvmPacket {
            source_client: "client-0".to_string(),
            dest_client: "07-tendermint-0".to_string(),
            sequence: PacketSequence(4),
            timeout_timestamp: TimeoutTimestamp(1_800_000_000),
            payloads: vec![EvmPayload {
                source_port: Port("transfer".to_string()),
                dest_port: Port("transfer".to_string()),
                version: "ics20-1".to_string(),
                encoding: "application/json".to_string(),
                value: vec![0xbb],
            }],
        }
    }

    fn proof() -> MerkleProof {
        MerkleProof {
            proof_bytes: vec![1, 2, 3],
        }
    }

    fn update_payload() -> CosmosUpdateClientPayload {
        CosmosUpdateClientPayload {
            headers: vec![],
            trusted_consensus_state: None,
            membership_proofs: mercury_core::MembershipProofs::new(),
        }
    }

    fn proof_entry(tag: u8) -> mercury_core::MembershipProofEntry {
        mercury_core::MembershipProofEntry {
            path: vec![vec![tag]],
            value: vec![tag],
            proof: vec![tag],
        }
    }

    async fn packet_messages(a: &EthereumAdapter) -> (EvmMessage, EvmMessage, EvmMessage) {
        let h = TmHeight::from(7_u32);
        let recv = PacketMessageBuilder::<Cosmos>::build_receive_packet_message(
            a,
            &cosmos_packet(),
            proof(),
            h,
            1,
        )
        .await
        .unwrap();
        let ack = PacketMessageBuilder::<Cosmos>::build_ack_packet_message(
            a,
            &cosmos_packet(),
            &PacketAcknowledgement(vec![9, 9]),
            proof(),
            h,
            1,
        )
        .await
        .unwrap();
        let timeout = PacketMessageBuilder::<Cosmos>::build_timeout_packet_message(
            a,
            &evm_packet(),
            proof(),
            h,
            1,
        )
        .await
        .unwrap();
        (recv, ack, timeout)
    }

    #[tokio::test]
    async fn packet_messages_carry_empty_proofs() {
        let a = adapter(no_rpc).await;
        let (recv, ack, timeout) = packet_messages(&a).await;

        let recv = ICS26Router::recvPacketCall::abi_decode(&recv.calldata).unwrap();
        assert!(recv.msg_.proofCommitment.is_empty());
        assert_eq!(recv.msg_.proofHeight.revisionNumber, 1);
        assert_eq!(recv.msg_.proofHeight.revisionHeight, 7);
        let p = &recv.msg_.packet;
        assert_eq!(p.sequence, 3);
        assert_eq!(p.sourceClient, "07-tendermint-0");
        assert_eq!(p.destClient, "client-0");
        assert_eq!(p.timeoutTimestamp, 1_900_000_000);
        assert_eq!(p.payloads[0].sourcePort, "transfer");
        assert_eq!(p.payloads[0].value.as_ref(), &[0xaa]);

        let ack = ICS26Router::ackPacketCall::abi_decode(&ack.calldata).unwrap();
        assert!(ack.msg_.proofAcked.is_empty());
        assert_eq!(ack.msg_.acknowledgement.as_ref(), &[9, 9]);
        assert_eq!(ack.msg_.packet.sequence, 3);

        let timeout = ICS26Router::timeoutPacketCall::abi_decode(&timeout.calldata).unwrap();
        assert!(timeout.msg_.proofTimeout.is_empty());
        let p = &timeout.msg_.packet;
        assert_eq!(p.sequence, 4);
        assert_eq!(p.sourceClient, "client-0");
        assert_eq!(p.destClient, "07-tendermint-0");
        assert_eq!(p.timeoutTimestamp, 1_800_000_000);
        assert_eq!(p.payloads[0].value.as_ref(), &[0xbb]);
    }

    #[tokio::test]
    async fn inject_membership_proof_targets_each_packet_kind() {
        let a = adapter(no_rpc).await;
        let (mut recv, mut ack, mut timeout) = packet_messages(&a).await;
        inject_membership_proof(&mut recv, &[7, 7]);
        inject_membership_proof(&mut ack, &[8]);
        inject_membership_proof(&mut timeout, &[9]);

        let recv = ICS26Router::recvPacketCall::abi_decode(&recv.calldata).unwrap();
        assert_eq!(recv.msg_.proofCommitment.as_ref(), &[7, 7]);
        assert_eq!(recv.msg_.packet.sequence, 3);
        let ack = ICS26Router::ackPacketCall::abi_decode(&ack.calldata).unwrap();
        assert_eq!(ack.msg_.proofAcked.as_ref(), &[8]);
        assert_eq!(ack.msg_.acknowledgement.as_ref(), &[9, 9]);
        let timeout = ICS26Router::timeoutPacketCall::abi_decode(&timeout.calldata).unwrap();
        assert_eq!(timeout.msg_.proofTimeout.as_ref(), &[9]);
    }

    #[test]
    fn inject_membership_proof_leaves_unknown_calldata() {
        let mut msg = EvmMessage {
            to: ROUTER,
            calldata: vec![0xde, 0xad, 0xbe, 0xef, 0x00],
            value: U256::ZERO,
        };
        inject_membership_proof(&mut msg, &[1]);
        assert_eq!(msg.calldata, vec![0xde, 0xad, 0xbe, 0xef, 0x00]);
    }

    #[tokio::test]
    async fn finalize_batch_injects_into_first_packet_only() {
        let a = adapter(no_rpc).await;
        let (recv, ack, _) = packet_messages(&a).await;
        let mut msgs = vec![recv, ack.clone()];
        let mut output = UpdateClientOutput {
            messages: vec![],
            membership_proof: Some(vec![5, 5]),
        };
        ClientMessageBuilder::<Cosmos>::finalize_batch(&a, &mut output, &mut msgs);

        assert!(output.membership_proof.is_none());
        let first = ICS26Router::recvPacketCall::abi_decode(&msgs[0].calldata).unwrap();
        assert_eq!(first.msg_.proofCommitment.as_ref(), &[5, 5]);
        assert_eq!(msgs[1].calldata, ack.calldata);
    }

    #[tokio::test]
    async fn finalize_batch_without_proof_or_packets() {
        let a = adapter(no_rpc).await;
        let (recv, _, _) = packet_messages(&a).await;
        let original = recv.calldata.clone();
        let mut msgs = vec![recv];
        let mut output = UpdateClientOutput::messages_only(vec![]);
        ClientMessageBuilder::<Cosmos>::finalize_batch(&a, &mut output, &mut msgs);
        assert_eq!(msgs[0].calldata, original);

        let mut output = UpdateClientOutput {
            messages: vec![],
            membership_proof: Some(vec![1]),
        };
        ClientMessageBuilder::<Cosmos>::finalize_batch(&a, &mut output, &mut []);
        assert!(output.membership_proof.is_none());
    }

    #[tokio::test]
    async fn enrich_update_payload_appends_proofs() {
        let a = adapter(no_rpc).await;
        let mut payload = update_payload();
        payload.membership_proofs.push(proof_entry(1));
        ClientMessageBuilder::<Cosmos>::enrich_update_payload(
            &a,
            &mut payload,
            &[proof_entry(2), proof_entry(3)],
        );
        let tags: Vec<u8> = payload
            .membership_proofs
            .0
            .iter()
            .map(|e| e.value[0])
            .collect();
        assert_eq!(tags, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn update_client_message_requires_sp1() {
        let a = adapter(no_rpc).await;
        let err = ClientMessageBuilder::<Cosmos>::build_update_client_message(
            &a,
            &EvmClientId("client-0".to_string()),
            update_payload(),
        )
        .await
        .err()
        .unwrap();
        assert!(
            err.to_string().contains("SP1 prover not configured"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn corrective_update_requires_sp1() {
        let a = adapter(no_rpc).await;
        let err = MisbehaviourMessageBuilder::<Cosmos>::build_misbehaviour_message(
            &a,
            &EvmClientId("client-0".to_string()),
            mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence::CorrectiveUpdate {
                payload: update_payload(),
            },
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("SP1 prover not configured"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn create_client_message_registers_light_client() {
        let a = adapter(no_rpc).await;
        let payload = CosmosCreateClientPayload {
            client_state: ibc_proto::google::protobuf::Any::default(),
            consensus_state: ibc_proto::google::protobuf::Any::default(),
            counterparty_client_id: Some("ignored".to_string()),
            counterparty_merkle_prefix: None,
        };
        let msg = ClientMessageBuilder::<Cosmos>::build_create_client_message(&a, payload)
            .await
            .unwrap();
        assert_eq!(msg.to, ROUTER);
        let call = ICS26Router::addClient_1Call::abi_decode(&msg.calldata).unwrap();
        assert_eq!(call.client, LIGHT_CLIENT);
        assert_eq!(call.counterpartyInfo.clientId, "");
        assert_eq!(call.counterpartyInfo.merklePrefix, Vec::<Bytes>::new());
    }

    #[tokio::test]
    async fn register_counterparty_message_uses_migrate_client() {
        let a = adapter(no_rpc).await;
        let cp: CosmosClientId = "08-wasm-12".parse().unwrap();
        let msg = ClientMessageBuilder::<Cosmos>::build_register_counterparty_message(
            &a,
            &EvmClientId("client-0".to_string()),
            &cp,
            MerklePrefix::ibc_default(),
        )
        .await
        .unwrap();
        let call = ICS26Router::migrateClientCall::abi_decode(&msg.calldata).unwrap();
        assert_eq!(call.clientId, "client-0");
        assert_eq!(call.counterpartyInfo.clientId, "08-wasm-12");
        assert_eq!(call.counterpartyInfo.merklePrefix.len(), 2);
        assert_eq!(call.client, LIGHT_CLIENT);
    }

    #[test]
    fn latest_height_and_trusting_period_from_sp1_state() {
        let cs = sp1_state(500, 3600);
        assert_eq!(
            <EthereumAdapter as ClientQuery<Cosmos>>::client_latest_height(&cs),
            TmHeight::from(500_u32)
        );
        assert_eq!(
            <EthereumAdapter as ClientQuery<Cosmos>>::trusting_period(&cs),
            Some(Duration::from_secs(3600))
        );
    }

    #[test]
    fn latest_height_falls_back_to_one() {
        let garbage = EvmClientState(vec![1, 2, 3]);
        assert_eq!(
            <EthereumAdapter as ClientQuery<Cosmos>>::client_latest_height(&garbage),
            TmHeight::from(1_u32)
        );
        assert_eq!(
            <EthereumAdapter as ClientQuery<Cosmos>>::trusting_period(&garbage),
            None
        );
        // TmHeight rejects values above i64::MAX.
        assert_eq!(
            <EthereumAdapter as ClientQuery<Cosmos>>::client_latest_height(&sp1_state(u64::MAX, 1)),
            TmHeight::from(1_u32)
        );
    }

    #[tokio::test]
    async fn update_payload_unwraps_wasm_client_state() {
        let a = adapter(no_rpc).await;
        let cs = CosmosClientState::Wasm(ibc_proto::ibc::lightclients::wasm::v1::ClientState {
            data: vec![],
            checksum: vec![],
            latest_height: None,
        });
        let payload = ClientPayloadBuilder::<Cosmos>::build_update_client_payload(
            &a,
            &EvmHeight(1),
            &EvmHeight(2),
            &cs,
        )
        .await
        .unwrap();
        assert_eq!(payload.headers.len(), 1);
    }

    #[tokio::test]
    async fn update_payload_rejects_non_wasm_client_state() {
        let a = adapter(no_rpc).await;
        let cs = attestations_client_state();
        let err = ClientPayloadBuilder::<Cosmos>::build_update_client_payload(
            &a,
            &EvmHeight(1),
            &EvmHeight(2),
            &cs,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("got Attestations"), "{err}");
    }

    #[tokio::test]
    async fn payload_heights_delegate_to_inner_chain() {
        let a = adapter(no_rpc).await;
        let payload = EvmUpdateClientPayload {
            headers: vec![],
            target_execution_height: Some(EvmHeight(10)),
            target_slot: Some(20),
            required_dst_timestamp_secs: None,
        };
        assert_eq!(
            ClientPayloadBuilder::<Cosmos>::update_payload_proof_height(&a, &payload),
            Some(EvmHeight(10))
        );
        assert_eq!(
            ClientPayloadBuilder::<Cosmos>::update_payload_message_height(&a, &payload),
            Some(EvmHeight(20))
        );
    }

    fn light_client_rpc(method: &str, params: &Value) -> Value {
        assert_eq!(method, "eth_call");
        let (to, input) = call_target(params);
        if to == ROUTER {
            return json!(B256::left_padding_from(LIGHT_CLIENT.as_slice()));
        }
        if SP1ICS07Tendermint::clientStateCall::abi_decode(&input).is_ok() {
            return json!(Bytes::from(sp1_state(321, 60).0));
        }
        let call = SP1ICS07Tendermint::getConsensusStateHashCall::abi_decode(&input).unwrap();
        assert_eq!(call.revisionHeight, 99);
        json!(B256::repeat_byte(0x42))
    }

    #[tokio::test]
    async fn cross_chain_client_and_consensus_state_queries() {
        let a = adapter(|m, p| Ok(light_client_rpc(m, p))).await;
        let id = EvmClientId("client-0".to_string());
        let cs = ClientQuery::<Cosmos>::query_client_state(&a, &id, &EvmHeight(0))
            .await
            .unwrap();
        assert_eq!(
            decode_client_state(&cs.0)
                .unwrap()
                .latestHeight
                .revisionHeight,
            321
        );

        let cons = ClientQuery::<Cosmos>::query_consensus_state(
            &a,
            &id,
            &TmHeight::from(99_u32),
            &EvmHeight(0),
        )
        .await
        .unwrap();
        assert_eq!(cons.0, vec![0x42; 32]);
    }

    #[tokio::test]
    async fn check_for_misbehaviour_is_noop() {
        let a = adapter(no_rpc).await;
        let cs = attestations_client_state();
        let cp: CosmosClientId = "07-tendermint-0".parse().unwrap();
        let result = MisbehaviourDetector::<Cosmos>::check_for_misbehaviour(&a, &cp, &(), &cs)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[expect(
        clippy::default_trait_access,
        reason = "the attestations client state type is not re-exported"
    )]
    fn attestations_client_state() -> CosmosClientState {
        CosmosClientState::Attestations(Default::default())
    }

    fn sol_consensus(ts: u128, byte: u8) -> IICS07TendermintMsgs::ConsensusState {
        IICS07TendermintMsgs::ConsensusState {
            timestamp: ts,
            root: B256::repeat_byte(byte),
            nextValidatorsHash: B256::repeat_byte(byte + 1),
        }
    }

    fn update_client_input(client_id: &str, new_height: u64, ts: u128) -> Bytes {
        let output = IUpdateClientMsgs::UpdateClientOutput {
            clientState: IICS07TendermintMsgs::ClientState {
                chainId: "cosmoshub-4".to_string(),
                trustLevel: IICS07TendermintMsgs::TrustThreshold {
                    numerator: 1,
                    denominator: 3,
                },
                latestHeight: SolIcs02::Height {
                    revisionNumber: 4,
                    revisionHeight: new_height,
                },
                trustingPeriod: 1,
                unbondingPeriod: 2,
                isFrozen: false,
                zkAlgorithm: IICS07TendermintMsgs::SupportedZkAlgorithm::Groth16,
            },
            trustedConsensusState: sol_consensus(1, 0x01),
            newConsensusState: sol_consensus(ts, 0x10),
            time: 0,
            trustedHeight: SolIcs02::Height {
                revisionNumber: 4,
                revisionHeight: 1,
            },
            newHeight: SolIcs02::Height {
                revisionNumber: 4,
                revisionHeight: new_height,
            },
        };
        let msg = IUpdateClientMsgs::MsgUpdateClient {
            sp1Proof: ISP1Msgs::SP1Proof {
                vKey: B256::ZERO,
                publicValues: <IUpdateClientMsgs::UpdateClientOutput as SolType>::abi_encode(
                    &output,
                )
                .into(),
                proof: Bytes::new(),
            },
        };
        ICS26Router::updateClientCall {
            clientId: client_id.to_string(),
            updateMsg: <IUpdateClientMsgs::MsgUpdateClient as SolType>::abi_encode(&msg).into(),
        }
        .abi_encode()
        .into()
    }

    fn rpc_tx(hash: B256, input: Bytes) -> Value {
        use alloy::consensus::{Signed, TxEnvelope, TxLegacy};
        let tx = TxLegacy {
            chain_id: Some(crate::test_support::CHAIN_ID),
            to: TxKind::Call(ROUTER),
            gas_limit: 100_000,
            input,
            ..Default::default()
        };
        let sig = alloy::primitives::Signature::new(U256::from(1), U256::from(1), false);
        let envelope = TxEnvelope::Legacy(Signed::new_unchecked(tx, sig, hash));
        serde_json::to_value(alloy::rpc::types::Transaction {
            inner: alloy::consensus::transaction::Recovered::new_unchecked(
                envelope,
                Address::repeat_byte(0x09),
            ),
            block_hash: Some(B256::repeat_byte(0x01)),
            block_number: Some(1),
            transaction_index: Some(0),
            effective_gas_price: None,
        })
        .unwrap()
    }

    fn client_updated_log(client_id: &str, tx_hash: B256) -> Value {
        let data = ICS26Router::ICS02ClientUpdated {
            clientId: client_id.to_string(),
            result: 0,
        }
        .encode_log_data();
        serde_json::to_value(alloy::rpc::types::Log {
            inner: alloy::primitives::Log {
                address: ROUTER,
                data,
            },
            block_number: Some(1),
            transaction_hash: Some(tx_hash),
            ..Default::default()
        })
        .unwrap()
    }

    #[tokio::test]
    async fn consensus_state_heights_decoded_from_update_txs_and_cached() {
        let h = B256::repeat_byte;
        let logs = json!([
            client_updated_log("client-0", h(0xa1)),
            client_updated_log("other-client", h(0xa2)),
            client_updated_log("client-0", h(0xa3)),
            client_updated_log("client-0", h(0xa4)),
            client_updated_log("client-0", h(0xa5)),
            client_updated_log("client-0", h(0xa6)),
        ]);
        let txs: std::collections::HashMap<B256, Value> = [
            (
                h(0xa1),
                rpc_tx(h(0xa1), update_client_input("client-0", 10, 1_000)),
            ),
            (
                h(0xa3),
                rpc_tx(h(0xa3), update_client_input("client-0", 30, 3_000)),
            ),
            (
                h(0xa4),
                rpc_tx(h(0xa4), update_client_input("client-0", 10, 1_000)),
            ),
            // Not an updateClient call: skipped.
            (h(0xa5), rpc_tx(h(0xa5), Bytes::from_static(&[1, 2, 3, 4]))),
            // Height above i64::MAX is not a valid Tendermint height: skipped.
            (
                h(0xa6),
                rpc_tx(h(0xa6), update_client_input("client-0", u64::MAX, 1)),
            ),
        ]
        .into_iter()
        .collect();
        let a = adapter(move |method, params| match method {
            "eth_getLogs" => {
                let topic0: B256 = params[0]["topics"][0].as_str().unwrap().parse().unwrap();
                assert_eq!(topic0, ICS26Router::ICS02ClientUpdated::SIGNATURE_HASH);
                Ok(logs.clone())
            }
            "eth_getTransactionByHash" => {
                let hash: B256 = params[0].as_str().unwrap().parse().unwrap();
                txs.get(&hash)
                    .cloned()
                    .ok_or_else(|| format!("unexpected tx {hash}"))
            }
            other => Err(format!("unexpected {other}")),
        })
        .await;

        let id = EvmClientId("client-0".to_string());
        let heights = MisbehaviourQuery::<Cosmos>::query_consensus_state_heights(&a, &id)
            .await
            .unwrap();
        assert_eq!(
            heights,
            vec![TmHeight::from(30_u32), TmHeight::from(10_u32)]
        );

        let cached = MisbehaviourQuery::<Cosmos>::query_update_client_header(
            &a,
            &id,
            &TmHeight::from(30_u32),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(cached.height, TmHeight::from(30_u32));
        assert_eq!(cached.timestamp_nanos, 3_000);
        assert_eq!(cached.root, [0x10; 32]);
        assert_eq!(cached.next_validators_hash, [0x11; 32]);

        let missing = MisbehaviourQuery::<Cosmos>::query_update_client_header(
            &a,
            &id,
            &TmHeight::from(20_u32),
        )
        .await
        .unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn consensus_state_heights_propagate_tx_fetch_errors() {
        let logs = json!([client_updated_log("client-0", B256::repeat_byte(1))]);
        let a = adapter(move |method, _| match method {
            "eth_getLogs" => Ok(logs.clone()),
            _ => Err("tx lookup failed".to_string()),
        })
        .await;
        let err = MisbehaviourQuery::<Cosmos>::query_consensus_state_heights(
            &a,
            &EvmClientId("client-0".to_string()),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("fetching updateClient transactions"),
            "{err:#}"
        );
    }
}
