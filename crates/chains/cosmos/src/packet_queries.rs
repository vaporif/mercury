use async_trait::async_trait;
use tracing::instrument;

use mercury_chain_traits::packet_queries::{
    CanQueryPacketAcknowledgement, CanQueryPacketCommitment, CanQueryPacketReceipt,
};
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::keys::CosmosSigner;
use crate::rpc::query_abci;
use crate::types::{MerkleProof, PacketAcknowledgement, PacketCommitment, PacketReceipt};

/// IBC v2 commitment key: `source_client_bytes` || 0x01 || `sequence_be_bytes`
fn commitment_key(source_client: &str, sequence: u64) -> Vec<u8> {
    let mut key = Vec::new();
    key.extend_from_slice(source_client.as_bytes());
    key.push(0x01);
    key.extend_from_slice(&sequence.to_be_bytes());
    key
}

/// IBC v2 receipt key: `dest_client_bytes` || 0x02 || `sequence_be_bytes`
fn receipt_key(dest_client: &str, sequence: u64) -> Vec<u8> {
    let mut key = Vec::new();
    key.extend_from_slice(dest_client.as_bytes());
    key.push(0x02);
    key.extend_from_slice(&sequence.to_be_bytes());
    key
}

/// IBC v2 ack key: `dest_client_bytes` || 0x03 || `sequence_be_bytes`
fn ack_key(dest_client: &str, sequence: u64) -> Vec<u8> {
    let mut key = Vec::new();
    key.extend_from_slice(dest_client.as_bytes());
    key.push(0x03);
    key.extend_from_slice(&sequence.to_be_bytes());
    key
}

fn extract_proof(response: &tendermint_rpc::endpoint::abci_query::AbciQuery) -> MerkleProof {
    let proof_bytes = response
        .proof
        .as_ref()
        .map(|proof_ops| {
            <tendermint::merkle::proof::ProofOps as tendermint_proto::Protobuf<
                tendermint_proto::v0_38::crypto::ProofOps,
            >>::encode_vec(proof_ops.clone())
        })
        .unwrap_or_default();
    MerkleProof { proof_bytes }
}

#[async_trait]
impl<S: CosmosSigner> CanQueryPacketCommitment<Self> for CosmosChain<S> {
    #[instrument(skip_all, name = "query_packet_commitment", fields(seq = sequence))]
    async fn query_packet_commitment(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<PacketCommitment>, MerkleProof)> {
        let response = query_abci(
            &self.rpc_client,
            "store/ibc/key",
            commitment_key(client_id.as_str(), sequence),
            Some(*height),
            true,
        )
        .await?;

        let proof = extract_proof(&response);
        let commitment = if response.value.is_empty() {
            None
        } else {
            Some(PacketCommitment(response.value))
        };
        Ok((commitment, proof))
    }
}

#[async_trait]
impl<S: CosmosSigner> CanQueryPacketReceipt<Self> for CosmosChain<S> {
    #[instrument(skip_all, name = "query_packet_receipt", fields(seq = sequence))]
    async fn query_packet_receipt(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<PacketReceipt>, MerkleProof)> {
        let response = query_abci(
            &self.rpc_client,
            "store/ibc/key",
            receipt_key(client_id.as_str(), sequence),
            Some(*height),
            true,
        )
        .await?;

        let proof = extract_proof(&response);
        let receipt = if response.value.is_empty() {
            None
        } else {
            Some(PacketReceipt)
        };
        Ok((receipt, proof))
    }
}

#[async_trait]
impl<S: CosmosSigner> CanQueryPacketAcknowledgement<Self> for CosmosChain<S> {
    #[instrument(skip_all, name = "query_packet_ack", fields(seq = sequence))]
    async fn query_packet_acknowledgement(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<PacketAcknowledgement>, MerkleProof)> {
        let response = query_abci(
            &self.rpc_client,
            "store/ibc/key",
            ack_key(client_id.as_str(), sequence),
            Some(*height),
            true,
        )
        .await?;

        let proof = extract_proof(&response);
        let ack = if response.value.is_empty() {
            None
        } else {
            Some(PacketAcknowledgement(response.value))
        };
        Ok((ack, proof))
    }
}
