use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::{HasChainTypes, HasIbcTypes};

/// Builds payloads for creating and updating IBC light clients.
#[async_trait]
pub trait CanBuildClientPayloads<Counterparty: HasChainTypes + ?Sized>: HasIbcTypes<Counterparty> {
    type CreateClientPayload: ThreadSafe;
    type UpdateClientPayload: ThreadSafe;

    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload>;

    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
    ) -> Result<Self::UpdateClientPayload>;
}

/// Builds messages for creating/updating IBC clients and registering counterparties.
#[async_trait]
pub trait CanBuildClientMessages<Counterparty>: HasIbcTypes<Counterparty>
where
    Counterparty: HasChainTypes + CanBuildClientPayloads<Self> + HasIbcTypes<Self>,
{
    async fn build_create_client_message(
        &self,
        payload: Counterparty::CreateClientPayload,
    ) -> Result<Self::Message>;

    async fn build_update_client_message(
        &self,
        client_id: &Self::ClientId,
        payload: Counterparty::UpdateClientPayload,
    ) -> Result<Vec<Self::Message>>;

    async fn build_register_counterparty_message(
        &self,
        client_id: &Self::ClientId,
        counterparty_client_id: &<Counterparty as HasIbcTypes<Self>>::ClientId,
    ) -> Result<Self::Message>;
}

/// Builds receive, ack, and timeout packet messages.
#[async_trait]
pub trait CanBuildPacketMessages<Counterparty>:
    HasIbcTypes<Counterparty>
where
    Counterparty: HasChainTypes + HasIbcTypes<Self>,
{
    type ReceivePacketPayload: ThreadSafe;
    type AckPacketPayload: ThreadSafe;
    type TimeoutPacketPayload: ThreadSafe;

    async fn build_receive_packet_message(
        &self,
        packet: &<Counterparty as HasIbcTypes<Self>>::Packet,
        payload: Self::ReceivePacketPayload,
    ) -> Result<Self::Message>;

    async fn build_ack_packet_message(
        &self,
        packet: &Self::Packet,
        ack: &<Counterparty as HasIbcTypes<Self>>::Acknowledgement,
        payload: Self::AckPacketPayload,
    ) -> Result<Self::Message>;

    async fn build_timeout_packet_message(
        &self,
        packet: &Self::Packet,
        payload: Self::TimeoutPacketPayload,
    ) -> Result<Self::Message>;
}
