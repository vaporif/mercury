use std::fmt::Debug;
use std::time::Duration;

use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;

use crate::types::ChainTypes;

/// Queries the current status (height and timestamp) of the chain.
#[async_trait]
pub trait ChainStatusQuery: ChainTypes {
    async fn query_chain_status(&self) -> Result<Self::ChainStatus>;

    async fn query_latest_height(&self) -> Result<Self::Height> {
        let status = self.query_chain_status().await?;
        Ok(Self::chain_status_height(&status).clone())
    }
}

/// Queries and inspects IBC client and consensus state.
#[async_trait]
pub trait ClientQuery<Counterparty: ChainTypes>: ChainTypes {
    type ClientState: Clone + Debug + ThreadSafe;
    type ConsensusState: Clone + Debug + ThreadSafe;

    async fn query_client_state(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Self::ClientState>;

    async fn query_consensus_state(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &Counterparty::Height,
        query_height: &Self::Height,
    ) -> Result<Self::ConsensusState>;

    fn trusting_period(client_state: &Self::ClientState) -> Option<Duration>;

    fn client_latest_height(client_state: &Self::ClientState) -> Counterparty::Height;
}

/// Queries consensus state heights and update headers for misbehaviour detection.
#[async_trait]
pub trait MisbehaviourQuery<Counterparty: ChainTypes>: ChainTypes {
    type CounterpartyUpdateHeader: ThreadSafe;

    /// List all consensus state heights for a client, in descending order.
    async fn query_consensus_state_heights(
        &self,
        client_id: &Self::ClientId,
    ) -> Result<Vec<Counterparty::Height>>;

    /// Returns the decoded header from the `UpdateClient` tx at the given consensus height.
    /// Returns None if the event has been pruned from the tx index.
    async fn query_update_client_header(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &Counterparty::Height,
    ) -> Result<Option<Self::CounterpartyUpdateHeader>>;
}

/// Queries packet commitments, receipts, and acknowledgements at a given height.
#[async_trait]
pub trait PacketStateQuery<Counterparty: ChainTypes>: ChainTypes {
    type PacketCommitment: ThreadSafe;
    type CommitmentProof: Clone + ThreadSafe;
    type PacketReceipt: ThreadSafe;
    type Acknowledgement: ThreadSafe;

    async fn query_packet_commitment(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::PacketCommitment>, Self::CommitmentProof)>;

    async fn query_packet_receipt(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::PacketReceipt>, Self::CommitmentProof)>;

    async fn query_packet_acknowledgement(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<Self::Acknowledgement>, Self::CommitmentProof)>;

    async fn query_commitment_sequences(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Vec<u64>>;
}
