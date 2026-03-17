use std::time::Duration;

use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::sol_types::{SolCall, SolEvent};
use async_trait::async_trait;
use eyre::Context;
use tracing::instrument;

use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use mercury_core::error::{ProofError, QueryError, Result};

use crate::chain::EthereumChainInner;
use crate::contracts::sp1_ics07;
use crate::contracts::{ICS26Router, SP1ICS07Tendermint};
use crate::ics24;
use crate::types::{
    EvmAcknowledgement, EvmChainStatus, EvmClientId, EvmClientState, EvmCommitmentProof,
    EvmConsensusState, EvmHeight, EvmPacketCommitment, EvmPacketReceipt, EvmTimestamp,
};

#[async_trait]
impl ChainStatusQuery for EthereumChainInner {
    async fn query_chain_status(&self) -> Result<EvmChainStatus> {
        let block_number = self
            .provider
            .get_block_number()
            .await
            .wrap_err("querying latest block number")?;

        let block_info = self
            .provider
            .get_block_by_number(block_number.into())
            .await
            .wrap_err("querying block by number")?
            .ok_or_else(|| QueryError::StaleState {
                what: format!("block {block_number}"),
            })?;

        Ok(EvmChainStatus {
            height: EvmHeight(block_number),
            timestamp: EvmTimestamp(block_info.header.timestamp),
        })
    }
}

pub async fn resolve_light_client(
    chain: &EthereumChainInner,
    client_id: &EvmClientId,
) -> Result<Address> {
    let router = ICS26Router::new(chain.router_address, &*chain.provider);
    router
        .getClient(client_id.0.clone())
        .call()
        .await
        .wrap_err_with(|| format!("getClient({client_id}) failed"))
}

pub type ClientStateReturn = sp1_ics07::SP1ICS07Tendermint::clientStateReturn;

/// Encode a decoded client state back to ABI bytes (matching the format
/// that `clientStateCall::abi_decode_returns` expects).
#[must_use]
pub fn encode_client_state(cs: &ClientStateReturn) -> Vec<u8> {
    sp1_ics07::SP1ICS07Tendermint::clientStateCall::abi_encode_returns(cs)
}

#[must_use]
pub fn decode_client_state(bytes: &[u8]) -> Option<ClientStateReturn> {
    tracing::debug!(
        bytes_len = bytes.len(),
        "decoding client state from ABI bytes"
    );
    let result = sp1_ics07::SP1ICS07Tendermint::clientStateCall::abi_decode_returns(bytes);
    match &result {
        Ok(cs) => {
            tracing::debug!(
                chain_id = %cs.chainId,
                revision_number = cs.latestHeight.revisionNumber,
                revision_height = cs.latestHeight.revisionHeight,
                trusting_period = cs.trustingPeriod,
                "decoded SP1 client state successfully"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, bytes_len = bytes.len(), "failed to decode SP1 client state");
        }
    }
    result.ok()
}

#[async_trait]
impl ClientQuery<Self> for EthereumChainInner {
    #[instrument(skip_all, name = "query_client_state", fields(client_id = %client_id))]
    async fn query_client_state(
        &self,
        client_id: &EvmClientId,
        _height: &EvmHeight,
    ) -> Result<EvmClientState> {
        let lc_address = resolve_light_client(self, client_id).await?;
        let lc = SP1ICS07Tendermint::new(lc_address, &*self.provider);
        // Use clientState() directly (struct accessor) instead of getClientState()
        // (bytes wrapper) to avoid ABI decode mismatches. Re-encode using
        // abi_encode_returns so decode_client_state can round-trip.
        let cs = lc
            .clientState()
            .call()
            .await
            .wrap_err("SP1ICS07Tendermint.clientState() failed")?;
        tracing::debug!(
            chain_id = %cs.chainId,
            revision_height = cs.latestHeight.revisionHeight,
            "queried SP1 client state"
        );
        Ok(EvmClientState(encode_client_state(&cs)))
    }

    #[instrument(skip_all, name = "query_consensus_state", fields(client_id = %client_id, consensus_height = %consensus_height))]
    async fn query_consensus_state(
        &self,
        client_id: &EvmClientId,
        consensus_height: &EvmHeight,
        _query_height: &EvmHeight,
    ) -> Result<EvmConsensusState> {
        let lc_address = resolve_light_client(self, client_id).await?;
        let lc = SP1ICS07Tendermint::new(lc_address, &*self.provider);
        let result = lc
            .getConsensusStateHash(consensus_height.0)
            .call()
            .await
            .wrap_err_with(|| {
                format!("getConsensusStateHash({consensus_height}) failed for client {client_id}")
            })?;
        Ok(EvmConsensusState(result.to_vec()))
    }

    fn trusting_period(client_state: &EvmClientState) -> Option<Duration> {
        let cs = decode_client_state(&client_state.0)?;
        Some(Duration::from_secs(u64::from(cs.trustingPeriod)))
    }

    fn client_latest_height(client_state: &EvmClientState) -> EvmHeight {
        decode_client_state(&client_state.0).map_or_else(
            || {
                tracing::warn!("failed to decode client state, defaulting to height 0");
                EvmHeight(0)
            },
            |cs| EvmHeight(cs.latestHeight.revisionHeight),
        )
    }
}

async fn get_storage_proof(
    chain: &EthereumChainInner,
    storage_slot: U256,
    height: &EvmHeight,
) -> Result<EvmCommitmentProof> {
    let block_id = alloy::eips::BlockId::number(height.0);
    let proof = chain
        .provider
        .get_proof(chain.router_address, vec![storage_slot.into()])
        .block_id(block_id)
        .await
        .wrap_err("eth_getProof failed")?;

    let sp = proof
        .storage_proof
        .first()
        .ok_or(ProofError::Missing)?;

    Ok(EvmCommitmentProof {
        proof_height: height.0,
        storage_root: proof.storage_hash,
        account_proof: proof.account_proof.iter().map(|b| b.to_vec()).collect(),
        storage_key: sp.key.as_b256(),
        storage_value: sp.value,
        storage_proof: sp.proof.iter().map(|b| b.to_vec()).collect(),
    })
}

async fn get_commitment_at_height(
    chain: &EthereumChainInner,
    hashed_path: B256,
    height: &EvmHeight,
) -> Result<B256> {
    let router = ICS26Router::new(chain.router_address, &*chain.provider);
    let result = router
        .getCommitment(hashed_path)
        .block(alloy::eips::BlockId::number(height.0))
        .call()
        .await
        .wrap_err("getCommitment failed")?;
    Ok(result)
}

/// Fetch commitment value + storage proof for a given ICS24 path key.
async fn query_commitment_with_proof(
    chain: &EthereumChainInner,
    hashed_path: B256,
    height: &EvmHeight,
) -> Result<(B256, EvmCommitmentProof)> {
    let storage_slot = ics24::commitment_storage_slot(hashed_path);
    let commitment_value = get_commitment_at_height(chain, hashed_path, height).await?;
    let proof = get_storage_proof(chain, storage_slot, height).await?;
    Ok((commitment_value, proof))
}

#[async_trait]
impl PacketStateQuery for EthereumChainInner {
    #[instrument(skip_all, name = "query_packet_commitment", fields(seq = sequence))]
    async fn query_packet_commitment(
        &self,
        client_id: &EvmClientId,
        sequence: u64,
        height: &EvmHeight,
    ) -> Result<(Option<EvmPacketCommitment>, EvmCommitmentProof)> {
        let hashed_path = ics24::packet_commitment_key(&client_id.0, sequence);
        let (value, proof) = query_commitment_with_proof(self, hashed_path, height).await?;
        let commitment = (!value.is_zero()).then(|| EvmPacketCommitment(value.to_vec()));
        Ok((commitment, proof))
    }

    #[instrument(skip_all, name = "query_packet_receipt", fields(seq = sequence))]
    async fn query_packet_receipt(
        &self,
        client_id: &EvmClientId,
        sequence: u64,
        height: &EvmHeight,
    ) -> Result<(Option<EvmPacketReceipt>, EvmCommitmentProof)> {
        let hashed_path = ics24::packet_receipt_key(&client_id.0, sequence);
        let (value, proof) = query_commitment_with_proof(self, hashed_path, height).await?;
        let receipt = (!value.is_zero()).then_some(EvmPacketReceipt);
        Ok((receipt, proof))
    }

    #[instrument(skip_all, name = "query_packet_ack", fields(seq = sequence))]
    async fn query_packet_acknowledgement(
        &self,
        client_id: &EvmClientId,
        sequence: u64,
        height: &EvmHeight,
    ) -> Result<(Option<EvmAcknowledgement>, EvmCommitmentProof)> {
        let hashed_path = ics24::ack_commitment_key(&client_id.0, sequence);
        let (value, proof) = query_commitment_with_proof(self, hashed_path, height).await?;
        let ack = (!value.is_zero()).then(|| EvmAcknowledgement(value.to_vec()));
        Ok((ack, proof))
    }

    #[instrument(skip_all, name = "query_commitment_sequences", fields(client_id = %client_id))]
    async fn query_commitment_sequences(
        &self,
        client_id: &EvmClientId,
        height: &EvmHeight,
    ) -> Result<Vec<u64>> {
        let filter = Filter::new()
            .address(self.router_address)
            .event_signature(ICS26Router::SendPacket::SIGNATURE_HASH)
            .from_block(self.config.deployment_block)
            .to_block(height.0);

        let logs = self
            .provider
            .get_logs(&filter)
            .await
            .wrap_err("querying SendPacket logs")?;

        let mut sequences: Vec<u64> = logs
            .iter()
            .filter_map(|log| {
                let decoded = ICS26Router::SendPacket::decode_log(log.as_ref()).ok()?;
                if decoded.data.packet.sourceClient == client_id.0 {
                    Some(decoded.data.packet.sequence)
                } else {
                    None
                }
            })
            .collect();

        sequences.sort_unstable();
        sequences.dedup();
        Ok(sequences)
    }
}
