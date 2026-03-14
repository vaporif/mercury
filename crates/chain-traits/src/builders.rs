use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::{ChainTypes, IbcTypes};

/// Builds payloads for creating and updating IBC light clients.
#[async_trait]
pub trait ClientPayloadBuilder<Counterparty: ChainTypes + ?Sized>: IbcTypes<Counterparty> {
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
pub trait ClientMessageBuilder<Counterparty>: IbcTypes<Counterparty>
where
    Counterparty: ChainTypes + ClientPayloadBuilder<Self> + IbcTypes<Self>,
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
        counterparty_client_id: &<Counterparty as IbcTypes<Self>>::ClientId,
        counterparty_merkle_prefix: Vec<Vec<u8>>,
    ) -> Result<Self::Message>;
}

/// Checks update headers against the source chain for light client divergence.
#[async_trait]
pub trait MisbehaviourDetector<Counterparty: ChainTypes + IbcTypes<Self> + ?Sized>:
    IbcTypes<Counterparty>
{
    type UpdateHeader: ThreadSafe;
    type MisbehaviourEvidence: ThreadSafe;

    /// Check a decoded update header against the source chain for divergence.
    /// `client_id` is the counterparty's client ID tracking this chain.
    /// Returns evidence if divergence detected, None if valid.
    async fn check_for_misbehaviour(
        &self,
        client_id: &<Counterparty as IbcTypes<Self>>::ClientId,
        update_header: &Self::UpdateHeader,
        client_state: &<Counterparty as IbcTypes<Self>>::ClientState,
    ) -> Result<Option<Self::MisbehaviourEvidence>>;
}

/// Builds a `MsgUpdateClient` containing misbehaviour evidence for submission on the destination chain.
#[async_trait]
pub trait MisbehaviourMessageBuilder<Counterparty>: IbcTypes<Counterparty>
where
    Counterparty: ChainTypes + MisbehaviourDetector<Self>,
{
    /// Build a `MsgUpdateClient` containing the misbehaviour evidence.
    async fn build_misbehaviour_message(
        &self,
        client_id: &Self::ClientId,
        evidence: Counterparty::MisbehaviourEvidence,
    ) -> Result<Self::Message>;
}

/// Builds receive, ack, and timeout packet messages.
#[async_trait]
pub trait PacketMessageBuilder<Counterparty>: IbcTypes<Counterparty>
where
    Counterparty: ChainTypes + IbcTypes<Self>,
{
    type ReceivePacketPayload: ThreadSafe;
    type AckPacketPayload: ThreadSafe;
    type TimeoutPacketPayload: ThreadSafe;

    async fn build_receive_packet_message(
        &self,
        packet: &<Counterparty as IbcTypes<Self>>::Packet,
        payload: Self::ReceivePacketPayload,
    ) -> Result<Self::Message>;

    async fn build_ack_packet_message(
        &self,
        packet: &Self::Packet,
        ack: &<Counterparty as IbcTypes<Self>>::Acknowledgement,
        payload: Self::AckPacketPayload,
    ) -> Result<Self::Message>;

    async fn build_timeout_packet_message(
        &self,
        packet: &Self::Packet,
        payload: Self::TimeoutPacketPayload,
    ) -> Result<Self::Message>;
}
