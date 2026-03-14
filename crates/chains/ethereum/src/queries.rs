use std::time::Duration;

use alloy::providers::Provider;
use async_trait::async_trait;
use eyre::Context;
use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use mercury_core::error::Result;

use crate::chain::EthereumChain;
use crate::types::{
    EvmAcknowledgement, EvmChainStatus, EvmClientId, EvmCommitmentProof, EvmHeight,
    EvmPacketCommitment, EvmPacketReceipt, EvmTimestamp,
};

#[async_trait]
impl ChainStatusQuery for EthereumChain {
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
            .ok_or_else(|| eyre::eyre!("block {block_number} not found"))?;

        Ok(EvmChainStatus {
            height: EvmHeight(block_number),
            timestamp: EvmTimestamp(block_info.header.timestamp),
        })
    }
}

#[async_trait]
impl ClientQuery<Self> for EthereumChain {
    async fn query_client_state(
        &self,
        _client_id: &EvmClientId,
        _height: &EvmHeight,
    ) -> Result<Vec<u8>> {
        todo!("query_client_state: read from light client contract")
    }

    async fn query_consensus_state(
        &self,
        _client_id: &EvmClientId,
        _consensus_height: &EvmHeight,
        _query_height: &EvmHeight,
    ) -> Result<Vec<u8>> {
        todo!("query_consensus_state: read from light client contract")
    }

    fn trusting_period(_client_state: &Vec<u8>) -> Option<Duration> {
        todo!("trusting_period: decode from client state bytes")
    }

    fn client_latest_height(_client_state: &Vec<u8>) -> EvmHeight {
        todo!("client_latest_height: decode from client state bytes")
    }
}

#[async_trait]
impl PacketStateQuery<Self> for EthereumChain {
    async fn query_packet_commitment(
        &self,
        _client_id: &EvmClientId,
        _sequence: u64,
        _height: &EvmHeight,
    ) -> Result<(Option<EvmPacketCommitment>, EvmCommitmentProof)> {
        todo!("query_packet_commitment: eth_getProof on ICS26Router storage")
    }

    async fn query_packet_receipt(
        &self,
        _client_id: &EvmClientId,
        _sequence: u64,
        _height: &EvmHeight,
    ) -> Result<(Option<EvmPacketReceipt>, EvmCommitmentProof)> {
        todo!("query_packet_receipt: eth_getProof on ICS26Router storage")
    }

    async fn query_packet_acknowledgement(
        &self,
        _client_id: &EvmClientId,
        _sequence: u64,
        _height: &EvmHeight,
    ) -> Result<(Option<EvmAcknowledgement>, EvmCommitmentProof)> {
        todo!("query_packet_acknowledgement: eth_getProof on ICS26Router storage")
    }

    async fn query_commitment_sequences(
        &self,
        _client_id: &EvmClientId,
        _height: &EvmHeight,
    ) -> Result<Vec<u64>> {
        todo!("query_commitment_sequences: scan SendPacket logs")
    }
}
