use std::time::Duration;

use async_trait::async_trait;
use tendermint::block::Height as TmHeight;

use mercury_chain_traits::builders::{
    ClientMessageBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder, PacketMessageBuilder,
    UpdateClientOutput,
};
use mercury_chain_traits::queries::{ClientQuery, MisbehaviourQuery};
use mercury_chain_traits::types::ChainTypes;
use mercury_core::error::Result;
use solana_sdk::signer::Signer;

use mercury_cosmos::builders::{CosmosCreateClientPayload, CosmosUpdateClientPayload};
use mercury_cosmos::chain::CosmosChain;
use mercury_cosmos::client_types::CosmosClientState;
use mercury_cosmos::keys::{CosmosSigner, Secp256k1KeyPair};
use mercury_cosmos::types::{CosmosPacket, MerkleProof, PacketAcknowledgement};

use mercury_solana::accounts::{
    self, fetch_account, resolve_ics07_program_id, OnChainRouterState,
};
use mercury_solana::chain::{SolanaChain, SolanaMisbehaviourEvidence};
use mercury_solana::instructions;
use mercury_solana::types::{
    SolanaClientId, SolanaClientState, SolanaConsensusState, SolanaHeight, SolanaMessage,
    SolanaPacket,
};

use crate::wrapper::SolanaAdapter;

async fn resolve_access_manager(chain: &SolanaChain) -> Result<solana_sdk::pubkey::Pubkey> {
    let (router_pda, _) = accounts::router_state_pda(&chain.ics26_program_id);
    let router: OnChainRouterState = fetch_account(&chain.rpc, &router_pda)
        .await?
        .ok_or_else(|| eyre::eyre!("router state PDA not found"))?;
    Ok(router.access_manager)
}

#[async_trait]
impl PacketMessageBuilder<CosmosChain<Secp256k1KeyPair>> for SolanaAdapter {
    async fn build_receive_packet_message(
        &self,
        packet: &CosmosPacket,
        proof: MerkleProof,
        proof_height: TmHeight,
        revision: u64,
    ) -> Result<SolanaMessage> {
        let chain = &self.0;
        let dest_client_id = &packet.dest_client_id.0;
        let dest_port = &packet.payloads.first()
            .ok_or_else(|| eyre::eyre!("packet has no payloads"))?.dest_port.0;
        let sequence = packet.sequence.0;

        let ics07 =
            resolve_ics07_program_id(&chain.rpc, dest_client_id, &chain.ics26_program_id).await?;
        let access_mgr = resolve_access_manager(chain).await?;
        let app_program =
            accounts::resolve_app_program_id(&chain.rpc, dest_port, &chain.ics26_program_id)
                .await?;

        let msg = instructions::MsgRecvPacket {
            packet_bytes: Vec::new(), // TODO: serialize packet in on-chain format
            proof_commitment: proof.proof_bytes,
            proof_height_revision_number: revision,
            proof_height_revision_height: proof_height.value(),
        };

        let ix = instructions::recv_packet(
            &chain.ics26_program_id,
            &chain.keypair.pubkey(),
            &msg,
            dest_client_id,
            dest_port,
            sequence,
            &ics07,
            proof_height.value(),
            &access_mgr,
            &app_program,
        );

        Ok(SolanaMessage {
            instructions: instructions::with_compute_budget(ix),
        })
    }

    async fn build_ack_packet_message(
        &self,
        packet: &CosmosPacket,
        ack: &PacketAcknowledgement,
        proof: MerkleProof,
        proof_height: TmHeight,
        revision: u64,
    ) -> Result<SolanaMessage> {
        let chain = &self.0;
        let source_client_id = &packet.source_client_id.0;
        let source_port = &packet.payloads.first()
            .ok_or_else(|| eyre::eyre!("packet has no payloads"))?.source_port.0;
        let sequence = packet.sequence.0;

        let ics07 =
            resolve_ics07_program_id(&chain.rpc, source_client_id, &chain.ics26_program_id)
                .await?;
        let access_mgr = resolve_access_manager(chain).await?;
        let app_program =
            accounts::resolve_app_program_id(&chain.rpc, source_port, &chain.ics26_program_id)
                .await?;

        let msg = instructions::MsgAckPacket {
            packet_bytes: Vec::new(), // TODO: serialize packet in on-chain format
            acknowledgement: ack.0.clone(),
            proof_acked: proof.proof_bytes,
            proof_height_revision_number: revision,
            proof_height_revision_height: proof_height.value(),
        };

        let ix = instructions::ack_packet(
            &chain.ics26_program_id,
            &chain.keypair.pubkey(),
            &msg,
            source_client_id,
            source_port,
            sequence,
            &ics07,
            proof_height.value(),
            &access_mgr,
            &app_program,
        );

        Ok(SolanaMessage {
            instructions: instructions::with_compute_budget(ix),
        })
    }

    async fn build_timeout_packet_message(
        &self,
        packet: &SolanaPacket,
        proof: MerkleProof,
        proof_height: TmHeight,
        revision: u64,
    ) -> Result<SolanaMessage> {
        let chain = &self.0;
        let source_client_id = &packet.source_client_id;
        let source_port = &packet.payloads.first()
            .ok_or_else(|| eyre::eyre!("packet has no payloads"))?.source_port.0;
        let sequence = packet.sequence.0;

        let ics07 =
            resolve_ics07_program_id(&chain.rpc, source_client_id, &chain.ics26_program_id)
                .await?;
        let access_mgr = resolve_access_manager(chain).await?;
        let app_program =
            accounts::resolve_app_program_id(&chain.rpc, source_port, &chain.ics26_program_id)
                .await?;

        let msg = instructions::MsgTimeoutPacket {
            packet_bytes: Vec::new(), // TODO: serialize packet in on-chain format
            proof_unreceived: proof.proof_bytes,
            proof_height_revision_number: revision,
            proof_height_revision_height: proof_height.value(),
        };

        let ix = instructions::timeout_packet(
            &chain.ics26_program_id,
            &chain.keypair.pubkey(),
            &msg,
            source_client_id,
            source_port,
            sequence,
            &ics07,
            proof_height.value(),
            &access_mgr,
            &app_program,
        );

        Ok(SolanaMessage {
            instructions: instructions::with_compute_budget(ix),
        })
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
        eyre::bail!("create_client requires ICS07 Initialize instruction format — not yet implemented")
    }

    async fn build_update_client_message(
        &self,
        _client_id: &SolanaClientId,
        _payload: CosmosUpdateClientPayload,
    ) -> Result<UpdateClientOutput<SolanaMessage>> {
        eyre::bail!("update_client requires Ed25519 sig extraction and header chunking — not yet implemented")
    }

    async fn build_register_counterparty_message(
        &self,
        client_id: &SolanaClientId,
        counterparty_client_id: &<CosmosChain<S> as ChainTypes>::ClientId,
        counterparty_merkle_prefix: mercury_core::MerklePrefix,
    ) -> Result<SolanaMessage> {
        let chain = &self.0;
        let access_mgr = resolve_access_manager(chain).await?;

        let ix = instructions::register_counterparty(
            &chain.ics26_program_id,
            &chain.keypair.pubkey(),
            &client_id.0,
            &counterparty_client_id.to_string(),
            &counterparty_merkle_prefix.0.concat(),
            &access_mgr,
        );

        Ok(SolanaMessage {
            instructions: vec![ix],
        })
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientQuery<CosmosChain<S>> for SolanaAdapter {
    async fn query_client_state(
        &self,
        client_id: &SolanaClientId,
        height: &SolanaHeight,
    ) -> Result<SolanaClientState> {
        <SolanaChain as ClientQuery<SolanaChain>>::query_client_state(&self.0, client_id, height)
            .await
    }

    async fn query_consensus_state(
        &self,
        client_id: &SolanaClientId,
        consensus_height: &TmHeight,
        query_height: &SolanaHeight,
    ) -> Result<SolanaConsensusState> {
        let solana_height = SolanaHeight(consensus_height.value());
        <SolanaChain as ClientQuery<SolanaChain>>::query_consensus_state(
            &self.0,
            client_id,
            &solana_height,
            query_height,
        )
        .await
    }

    fn trusting_period(client_state: &SolanaClientState) -> Option<Duration> {
        <SolanaChain as ClientQuery<SolanaChain>>::trusting_period(client_state)
    }

    fn client_latest_height(client_state: &SolanaClientState) -> TmHeight {
        let h = <SolanaChain as ClientQuery<SolanaChain>>::client_latest_height(client_state);
        TmHeight::try_from(h.0).expect("height conversion")
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
        tracing::debug!("misbehaviour detection not yet implemented for Cosmos-on-Solana");
        Ok(None)
    }
}

#[async_trait]
impl<S: CosmosSigner> MisbehaviourQuery<CosmosChain<S>> for SolanaAdapter {
    type CounterpartyUpdateHeader = ibc_client_tendermint::types::Header;

    async fn query_consensus_state_heights(
        &self,
        _client_id: &SolanaClientId,
    ) -> Result<Vec<TmHeight>> {
        Ok(Vec::new())
    }

    async fn query_update_client_header(
        &self,
        _client_id: &SolanaClientId,
        _consensus_height: &TmHeight,
    ) -> Result<Option<ibc_client_tendermint::types::Header>> {
        Ok(None)
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
        eyre::bail!("misbehaviour submission not yet implemented for Cosmos-on-Solana")
    }
}
