use std::fmt::Debug;

use async_trait::async_trait;
use mercury_core::error::Result;
use mercury_core::{MerklePrefix, ThreadSafe};

use crate::types::{ChainTypes, IbcTypes};

/// Result of `build_update_client_message`, carrying both update messages
/// and an optional combined membership proof for injection into packet messages.
pub struct UpdateClientOutput<M> {
    pub messages: Vec<M>,
    /// ABI-encoded combined membership proof. When present, the client update
    /// happens implicitly during on-chain membership verification, so
    /// `messages` may be empty for the header covered by the combined proof.
    pub membership_proof: Option<Vec<u8>>,
}

impl<M> UpdateClientOutput<M> {
    #[must_use]
    pub const fn messages_only(messages: Vec<M>) -> Self {
        Self {
            messages,
            membership_proof: None,
        }
    }
}

/// Builds payloads for creating and updating IBC light clients.
#[async_trait]
pub trait ClientPayloadBuilder<Counterparty: ChainTypes>: ChainTypes {
    type CreateClientPayload: ThreadSafe;
    type UpdateClientPayload: ThreadSafe;

    async fn build_create_client_payload(&self) -> Result<Self::CreateClientPayload>;

    async fn build_update_client_payload(
        &self,
        trusted_height: &Self::Height,
        target_height: &Self::Height,
        counterparty_client_state: &<Counterparty as IbcTypes>::ClientState,
    ) -> Result<Self::UpdateClientPayload>
    where
        Counterparty: IbcTypes;
}

/// Builds messages for creating/updating IBC clients and registering counterparties.
#[async_trait]
pub trait ClientMessageBuilder<Counterparty: ChainTypes>: IbcTypes {
    type CreateClientPayload: ThreadSafe;
    type UpdateClientPayload: ThreadSafe;

    async fn build_create_client_message(
        &self,
        payload: Self::CreateClientPayload,
    ) -> Result<Self::Message>;

    async fn build_update_client_message(
        &self,
        client_id: &Self::ClientId,
        payload: Self::UpdateClientPayload,
    ) -> Result<UpdateClientOutput<Self::Message>>;

    async fn build_register_counterparty_message(
        &self,
        client_id: &Self::ClientId,
        counterparty_client_id: &Counterparty::ClientId,
        counterparty_merkle_prefix: MerklePrefix,
    ) -> Result<Self::Message>;

    /// Enriches the update client payload with membership proof entries for batched proving.
    /// Called before `build_update_client_message`. Default is a no-op.
    fn enrich_update_payload(
        &self,
        _payload: &mut Self::UpdateClientPayload,
        _proofs: &[mercury_core::MembershipProofEntry],
    ) {
    }

    /// Post-process a batch of update and packet messages before sending.
    /// Called after both update and packet messages are built. Default is a no-op.
    fn finalize_batch(
        &self,
        _update_output: &mut UpdateClientOutput<Self::Message>,
        _packet_messages: &mut [Self::Message],
    ) {
    }
}

/// Checks update headers against the source chain for light client divergence.
#[async_trait]
pub trait MisbehaviourDetector<Counterparty: ChainTypes>: IbcTypes {
    type UpdateHeader: ThreadSafe;
    type MisbehaviourEvidence: ThreadSafe;
    type CounterpartyClientState: Clone + Debug + ThreadSafe;

    /// Check a decoded update header against the source chain for divergence.
    /// `client_id` is the counterparty's client ID tracking this chain.
    /// Returns evidence if divergence detected, None if valid.
    async fn check_for_misbehaviour(
        &self,
        client_id: &Counterparty::ClientId,
        update_header: &Self::UpdateHeader,
        client_state: &Self::CounterpartyClientState,
    ) -> Result<Option<Self::MisbehaviourEvidence>>;
}

/// Builds a `MsgUpdateClient` containing misbehaviour evidence for submission on the destination chain.
#[async_trait]
pub trait MisbehaviourMessageBuilder<Counterparty: ChainTypes>: IbcTypes {
    type MisbehaviourEvidence: ThreadSafe;

    /// Build a `MsgUpdateClient` containing the misbehaviour evidence.
    async fn build_misbehaviour_message(
        &self,
        client_id: &Self::ClientId,
        evidence: Self::MisbehaviourEvidence,
    ) -> Result<Self::Message>;
}

/// Builds receive, ack, and timeout packet messages.
#[async_trait]
pub trait PacketMessageBuilder<Counterparty: IbcTypes>: IbcTypes {
    async fn build_receive_packet_message(
        &self,
        packet: &<Counterparty as IbcTypes>::Packet,
        proof: <Counterparty as IbcTypes>::CommitmentProof,
        proof_height: <Counterparty as ChainTypes>::Height,
        revision: u64,
    ) -> Result<Self::Message>;

    async fn build_ack_packet_message(
        &self,
        packet: &<Counterparty as IbcTypes>::Packet,
        ack: &<Counterparty as IbcTypes>::Acknowledgement,
        proof: <Counterparty as IbcTypes>::CommitmentProof,
        proof_height: <Counterparty as ChainTypes>::Height,
        revision: u64,
    ) -> Result<Self::Message>;

    async fn build_timeout_packet_message(
        &self,
        packet: &Self::Packet,
        proof: <Counterparty as IbcTypes>::CommitmentProof,
        proof_height: <Counterparty as ChainTypes>::Height,
        revision: u64,
    ) -> Result<Self::Message>;
}
