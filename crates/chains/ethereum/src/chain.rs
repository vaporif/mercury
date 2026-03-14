use std::sync::Arc;
use std::time::Duration;

use alloy::network::EthereumWallet;
use alloy::primitives::Address;
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use async_trait::async_trait;
use eyre::Context;
use mercury_chain_traits::builders::{ClientMessageBuilder, ClientPayloadBuilder};
use mercury_chain_traits::types::{ChainTypes, IbcTypes};

use crate::builders::{CreateClientPayload, UpdateClientPayload};
use crate::config::EthereumChainConfig;
use crate::types::{
    EvmAcknowledgement, EvmChainId, EvmChainStatus, EvmClientId, EvmCommitmentProof, EvmEvent,
    EvmHeight, EvmMessage, EvmPacket, EvmPacketCommitment, EvmPacketReceipt, EvmTimestamp,
    EvmTxResponse,
};

#[derive(Clone)]
pub struct EthereumChain {
    pub config: EthereumChainConfig,
    pub chain_id: EvmChainId,
    pub router_address: Address,
    pub provider: Arc<DynProvider>,
}

impl std::fmt::Debug for EthereumChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthereumChain")
            .field("chain_id", &self.chain_id)
            .field("router_address", &self.router_address)
            .finish_non_exhaustive()
    }
}

impl EthereumChain {
    pub async fn new(
        config: EthereumChainConfig,
        signer: alloy::signers::local::PrivateKeySigner,
    ) -> mercury_core::error::Result<Self> {
        let wallet = EthereumWallet::from(signer);
        let url: url::Url = config.rpc_addr.parse().wrap_err("parsing RPC URL")?;
        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .connect_http(url)
            .erased();

        let on_chain_id: u64 = provider
            .get_chain_id()
            .await
            .wrap_err("querying chain ID")?;

        if on_chain_id != config.chain_id {
            eyre::bail!(
                "chain_id mismatch: config says {}, node reports {}",
                config.chain_id,
                on_chain_id,
            );
        }

        let router_address = config.router_address()?;

        Ok(Self {
            chain_id: EvmChainId(config.chain_id),
            config,
            router_address,
            provider: Arc::new(provider),
        })
    }
}

impl ChainTypes for EthereumChain {
    type Height = EvmHeight;
    type Timestamp = EvmTimestamp;
    type ChainId = EvmChainId;
    type Event = EvmEvent;
    type Message = EvmMessage;
    type MessageResponse = EvmTxResponse;
    type ChainStatus = EvmChainStatus;

    fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height {
        &status.height
    }

    fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp {
        &status.timestamp
    }

    fn chain_status_timestamp_secs(status: &Self::ChainStatus) -> u64 {
        status.timestamp.0
    }

    fn revision_number(&self) -> u64 {
        0
    }

    fn increment_height(height: &EvmHeight) -> Option<EvmHeight> {
        height.0.checked_add(1).map(EvmHeight)
    }

    fn sub_height(height: &EvmHeight, n: u64) -> Option<EvmHeight> {
        Some(EvmHeight(height.0.saturating_sub(n).max(1)))
    }

    fn block_time(&self) -> Duration {
        self.config.block_time()
    }
}

impl IbcTypes<Self> for EthereumChain {
    type ClientId = EvmClientId;
    type ClientState = Vec<u8>;
    type ConsensusState = Vec<u8>;
    type CommitmentProof = EvmCommitmentProof;
    type Packet = EvmPacket;
    type PacketCommitment = EvmPacketCommitment;
    type PacketReceipt = EvmPacketReceipt;
    type Acknowledgement = EvmAcknowledgement;

    fn packet_sequence(packet: &EvmPacket) -> u64 {
        packet.sequence
    }

    fn packet_timeout_timestamp(packet: &EvmPacket) -> u64 {
        packet.timeout_timestamp
    }

    fn packet_source_ports(packet: &EvmPacket) -> Vec<String> {
        packet
            .payloads
            .iter()
            .map(|p| p.source_port.clone())
            .collect()
    }
}

#[async_trait]
impl ClientPayloadBuilder<Self> for EthereumChain {
    type CreateClientPayload = CreateClientPayload;
    type UpdateClientPayload = UpdateClientPayload;

    async fn build_create_client_payload(
        &self,
    ) -> mercury_core::error::Result<CreateClientPayload> {
        todo!("build_create_client_payload for EVM chain")
    }

    async fn build_update_client_payload(
        &self,
        _trusted_height: &EvmHeight,
        _target_height: &EvmHeight,
    ) -> mercury_core::error::Result<UpdateClientPayload> {
        todo!("build_update_client_payload for EVM chain")
    }
}

#[async_trait]
impl ClientMessageBuilder<Self> for EthereumChain {
    async fn build_create_client_message(
        &self,
        _payload: CreateClientPayload,
    ) -> mercury_core::error::Result<EvmMessage> {
        todo!("build_create_client_message for EVM chain")
    }

    async fn build_update_client_message(
        &self,
        _client_id: &EvmClientId,
        _payload: UpdateClientPayload,
    ) -> mercury_core::error::Result<Vec<EvmMessage>> {
        todo!("build_update_client_message for EVM chain")
    }

    async fn build_register_counterparty_message(
        &self,
        _client_id: &EvmClientId,
        _counterparty_client_id: &EvmClientId,
    ) -> mercury_core::error::Result<EvmMessage> {
        todo!("build_register_counterparty_message for EVM chain")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increment_height() {
        let h = EvmHeight(100);
        assert_eq!(EthereumChain::increment_height(&h), Some(EvmHeight(101)));
    }

    #[test]
    fn increment_height_overflow() {
        let h = EvmHeight(u64::MAX);
        assert_eq!(EthereumChain::increment_height(&h), None);
    }

    #[test]
    fn sub_height_normal() {
        let h = EvmHeight(10);
        assert_eq!(EthereumChain::sub_height(&h, 3), Some(EvmHeight(7)));
    }

    #[test]
    fn sub_height_clamps_to_one() {
        let h = EvmHeight(5);
        assert_eq!(EthereumChain::sub_height(&h, 100), Some(EvmHeight(1)));
    }

    #[test]
    fn chain_status_extracts() {
        let status = EvmChainStatus {
            height: EvmHeight(42),
            timestamp: EvmTimestamp(1_700_000_000),
        };
        assert_eq!(EthereumChain::chain_status_height(&status).0, 42);
        assert_eq!(
            EthereumChain::chain_status_timestamp_secs(&status),
            1_700_000_000
        );
    }

    #[test]
    fn packet_sequence_extracts() {
        let packet = EvmPacket {
            source_client: "client-0".to_string(),
            dest_client: "client-1".to_string(),
            sequence: 42,
            timeout_timestamp: 0,
            payloads: vec![],
        };
        assert_eq!(EthereumChain::packet_sequence(&packet), 42);
    }

    #[test]
    fn packet_timeout_extracts() {
        let packet = EvmPacket {
            source_client: "client-0".to_string(),
            dest_client: "client-1".to_string(),
            sequence: 1,
            timeout_timestamp: 1_700_000_000,
            payloads: vec![],
        };
        assert_eq!(
            EthereumChain::packet_timeout_timestamp(&packet),
            1_700_000_000
        );
    }
}
