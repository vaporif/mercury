//! Cross-chain trait implementations for Cosmos → EVM client relay.
//! Gated behind the `cosmos-sp1` feature.

use std::time::Duration;

use alloy::primitives::U256;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use eyre::Context;
use tendermint::block::Height as TmHeight;
use tracing::instrument;

use mercury_chain_traits::builders::{ClientMessageBuilder, PacketMessageBuilder};
use mercury_chain_traits::queries::ClientQuery;
use mercury_core::MerklePrefix;
use mercury_core::error::Result;

use mercury_cosmos::builders::{CosmosCreateClientPayload, CosmosUpdateClientPayload};
use mercury_cosmos::chain::CosmosChainInner;
use mercury_cosmos::keys::CosmosSigner;
use mercury_cosmos::types::{CosmosPacket, MerkleProof, PacketAcknowledgement};

use crate::wrapper::EthereumChain;
use mercury_ethereum::chain::to_sol_merkle_prefix;
use mercury_ethereum::contracts::{
    ICS26Router, IICS02ClientMsgs, IICS26RouterMsgs, SP1ICS07Tendermint,
};
use mercury_ethereum::queries::{decode_client_state, resolve_light_client};
use mercury_ethereum::types::{
    EvmClientId, EvmClientState, EvmConsensusState, EvmHeight, EvmMessage, EvmPacket,
};

#[async_trait]
impl<S: CosmosSigner> ClientQuery<CosmosChainInner<S>> for EthereumChain {
    #[instrument(skip_all, name = "query_client_state", fields(client_id = %client_id))]
    async fn query_client_state(
        &self,
        client_id: &EvmClientId,
        _height: &EvmHeight,
    ) -> Result<EvmClientState> {
        let lc_address = resolve_light_client(&self.0, client_id).await?;
        let lc = SP1ICS07Tendermint::new(lc_address, &*self.provider);
        let result = lc
            .getClientState()
            .call()
            .await
            .wrap_err("SP1ICS07Tendermint.getClientState() failed")?;
        Ok(EvmClientState(result.to_vec()))
    }

    #[instrument(skip_all, name = "query_consensus_state", fields(client_id = %client_id, consensus_height = %consensus_height))]
    async fn query_consensus_state(
        &self,
        client_id: &EvmClientId,
        consensus_height: &TmHeight,
        _query_height: &EvmHeight,
    ) -> Result<EvmConsensusState> {
        let height_u64 = consensus_height.value();
        let lc_address = resolve_light_client(&self.0, client_id).await?;
        let lc = SP1ICS07Tendermint::new(lc_address, &*self.provider);
        let result = lc
            .getConsensusStateHash(height_u64)
            .call()
            .await
            .wrap_err_with(|| format!("getConsensusStateHash({height_u64}) failed"))?;
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
                TmHeight::try_from(1u64).expect("height 1 is valid")
            },
            |cs| {
                TmHeight::try_from(cs.latestHeight.revisionHeight)
                    .unwrap_or_else(|_| TmHeight::try_from(1u64).expect("height 1 is valid"))
            },
        )
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientMessageBuilder<CosmosChainInner<S>> for EthereumChain {
    type CreateClientPayload = CosmosCreateClientPayload;
    type UpdateClientPayload = CosmosUpdateClientPayload;

    async fn build_create_client_message(
        &self,
        _payload: CosmosCreateClientPayload,
    ) -> Result<EvmMessage> {
        todo!("cross-chain create client not yet implemented")
    }

    async fn build_update_client_message(
        &self,
        client_id: &EvmClientId,
        payload: CosmosUpdateClientPayload,
    ) -> Result<Vec<EvmMessage>> {
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
                sp1,
            )
            .await
    }

    async fn build_register_counterparty_message(
        &self,
        client_id: &EvmClientId,
        counterparty_client_id: &<CosmosChainInner<S> as mercury_chain_traits::types::ChainTypes>::ClientId,
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
}

fn cosmos_packet_to_sol(packet: &CosmosPacket) -> IICS26RouterMsgs::Packet {
    IICS26RouterMsgs::Packet {
        sequence: packet.sequence,
        sourceClient: packet.source_client_id.to_string(),
        destClient: packet.dest_client_id.to_string(),
        timeoutTimestamp: packet.timeout_timestamp,
        payloads: packet
            .payloads
            .iter()
            .map(|p| IICS26RouterMsgs::Payload {
                sourcePort: p.source_port.clone(),
                destPort: p.dest_port.clone(),
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
impl<S: CosmosSigner> PacketMessageBuilder<CosmosChainInner<S>> for EthereumChain {
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
            sequence: packet.sequence,
            sourceClient: packet.source_client.clone(),
            destClient: packet.dest_client.clone(),
            timeoutTimestamp: packet.timeout_timestamp,
            payloads: packet
                .payloads
                .iter()
                .map(|p| IICS26RouterMsgs::Payload {
                    sourcePort: p.source_port.clone(),
                    destPort: p.dest_port.clone(),
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
        EthereumChain:
            IbcTypes + ClientQuery<CosmosChainInner<S>> + ClientMessageBuilder<CosmosChainInner<S>>,
        CosmosChainInner<S>: ClientPayloadBuilder<EthereumChain>,
    {
    }
}
