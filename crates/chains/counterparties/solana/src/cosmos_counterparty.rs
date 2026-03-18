use async_trait::async_trait;
use tendermint::block::Height as TmHeight;

use mercury_chain_traits::builders::{
    ClientMessageBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder, PacketMessageBuilder,
    UpdateClientOutput,
};
use mercury_chain_traits::queries::{ClientQuery, MisbehaviourQuery};
use mercury_chain_traits::types::ChainTypes;
use mercury_core::error::Result;

use mercury_cosmos::builders::{CosmosCreateClientPayload, CosmosUpdateClientPayload};
use mercury_cosmos::chain::CosmosChain;
use mercury_cosmos::client_types::CosmosClientState;
use mercury_cosmos::keys::{CosmosSigner, Secp256k1KeyPair};
use mercury_cosmos::types::{CosmosPacket, MerkleProof, PacketAcknowledgement};

use mercury_solana::chain::SolanaMisbehaviourEvidence;
use mercury_solana::types::{
    SolanaClientId, SolanaClientState, SolanaConsensusState, SolanaHeight, SolanaMessage,
    SolanaPacket,
};

use crate::wrapper::SolanaAdapter;

#[async_trait]
impl PacketMessageBuilder<CosmosChain<Secp256k1KeyPair>> for SolanaAdapter {
    async fn build_receive_packet_message(
        &self,
        _packet: &CosmosPacket,
        _proof: MerkleProof,
        _proof_height: TmHeight,
        _revision: u64,
    ) -> Result<SolanaMessage> {
        todo!("build Solana recv_packet instruction with Cosmos Merkle proof")
    }

    async fn build_ack_packet_message(
        &self,
        _packet: &CosmosPacket,
        _ack: &PacketAcknowledgement,
        _proof: MerkleProof,
        _proof_height: TmHeight,
        _revision: u64,
    ) -> Result<SolanaMessage> {
        todo!("build Solana ack_packet instruction with Cosmos Merkle proof")
    }

    async fn build_timeout_packet_message(
        &self,
        _packet: &SolanaPacket,
        _proof: MerkleProof,
        _proof_height: TmHeight,
        _revision: u64,
    ) -> Result<SolanaMessage> {
        todo!("build Solana timeout_packet instruction with Cosmos receipt proof")
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientMessageBuilder<CosmosChain<S>> for SolanaAdapter {
    type CreateClientPayload = CosmosCreateClientPayload;
    type UpdateClientPayload = CosmosUpdateClientPayload;

    async fn build_create_client_message(
        &self,
        _payload: CosmosCreateClientPayload,
    ) -> Result<SolanaMessage> {
        todo!("build Solana create_client instruction for Cosmos light client")
    }

    async fn build_update_client_message(
        &self,
        _client_id: &SolanaClientId,
        _payload: CosmosUpdateClientPayload,
    ) -> Result<UpdateClientOutput<SolanaMessage>> {
        todo!("build Solana update_client instruction for Cosmos light client")
    }

    async fn build_register_counterparty_message(
        &self,
        _client_id: &SolanaClientId,
        _counterparty_client_id: &<CosmosChain<S> as ChainTypes>::ClientId,
        _counterparty_merkle_prefix: mercury_core::MerklePrefix,
    ) -> Result<SolanaMessage> {
        todo!("build Solana register_counterparty instruction")
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientQuery<CosmosChain<S>> for SolanaAdapter {
    async fn query_client_state(
        &self,
        _client_id: &SolanaClientId,
        _height: &SolanaHeight,
    ) -> Result<SolanaClientState> {
        todo!("query Cosmos light client state from Solana program account")
    }

    async fn query_consensus_state(
        &self,
        _client_id: &SolanaClientId,
        _consensus_height: &TmHeight,
        _query_height: &SolanaHeight,
    ) -> Result<SolanaConsensusState> {
        todo!("query Cosmos consensus state from Solana program account")
    }

    fn trusting_period(_client_state: &SolanaClientState) -> Option<std::time::Duration> {
        todo!("extract trusting period from Cosmos client state on Solana")
    }

    fn client_latest_height(_client_state: &SolanaClientState) -> TmHeight {
        todo!("extract latest Cosmos height from client state on Solana")
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourDetector<CosmosChain<S>> for SolanaAdapter {
    type UpdateHeader = ibc_client_tendermint::types::Header;
    type MisbehaviourEvidence = SolanaMisbehaviourEvidence;
    type CounterpartyClientState = CosmosClientState;

    async fn check_for_misbehaviour(
        &self,
        _client_id: &<CosmosChain<S> as ChainTypes>::ClientId,
        _update_header: &Self::UpdateHeader,
        _client_state: &Self::CounterpartyClientState,
    ) -> Result<Option<Self::MisbehaviourEvidence>> {
        todo!("check Cosmos headers for misbehaviour from Solana perspective")
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourQuery<CosmosChain<S>> for SolanaAdapter {
    type CounterpartyUpdateHeader = ibc_client_tendermint::types::Header;

    async fn query_consensus_state_heights(
        &self,
        _client_id: &SolanaClientId,
    ) -> Result<Vec<TmHeight>> {
        todo!("query Cosmos consensus state heights from Solana program")
    }

    async fn query_update_client_header(
        &self,
        _client_id: &SolanaClientId,
        _consensus_height: &TmHeight,
    ) -> Result<Option<ibc_client_tendermint::types::Header>> {
        todo!("query update client header from Solana transaction history")
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourMessageBuilder<CosmosChain<S>> for SolanaAdapter {
    type MisbehaviourEvidence = mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence;

    async fn build_misbehaviour_message(
        &self,
        _client_id: &SolanaClientId,
        _evidence: mercury_cosmos::misbehaviour::CosmosMisbehaviourEvidence,
    ) -> Result<SolanaMessage> {
        todo!("build Solana misbehaviour submission instruction")
    }
}
