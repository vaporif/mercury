use async_trait::async_trait;

use mercury_chain_traits::packet_queries::{
    CanQueryPacketAcknowledgement, CanQueryPacketCommitment, CanQueryPacketReceipt,
};
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::rpc::query_abci;
use crate::types::{MerkleProof, PacketAcknowledgement, PacketCommitment, PacketReceipt};

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
impl CanQueryPacketCommitment<Self> for CosmosChain {
    async fn query_packet_commitment(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<PacketCommitment>, MerkleProof)> {
        let key = format!("ibc/{client_id}/commitments/{sequence}");
        let response = query_abci(
            &self.rpc_client,
            "store/ibc/key",
            key.into_bytes(),
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
impl CanQueryPacketReceipt<Self> for CosmosChain {
    async fn query_packet_receipt(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<PacketReceipt>, MerkleProof)> {
        let key = format!("ibc/{client_id}/receipts/{sequence}");
        let response = query_abci(
            &self.rpc_client,
            "store/ibc/key",
            key.into_bytes(),
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
impl CanQueryPacketAcknowledgement<Self> for CosmosChain {
    async fn query_packet_acknowledgement(
        &self,
        client_id: &Self::ClientId,
        sequence: u64,
        height: &Self::Height,
    ) -> Result<(Option<PacketAcknowledgement>, MerkleProof)> {
        let key = format!("ibc/{client_id}/acks/{sequence}");
        let response = query_abci(
            &self.rpc_client,
            "store/ibc/key",
            key.into_bytes(),
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
