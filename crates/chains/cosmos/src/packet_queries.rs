use async_trait::async_trait;
use prost::Message;
use tracing::{instrument, warn};

use mercury_chain_traits::packet_queries::{
    CanQueryPacketAcknowledgement, CanQueryPacketCommitment, CanQueryPacketReceipt,
};
use mercury_core::error::Result;
use tendermint::block::Height as TmHeight;

use crate::chain::CosmosChain;
use crate::keys::CosmosSigner;
use crate::rpc::query_abci;
use crate::types::{MerkleProof, PacketAcknowledgement, PacketCommitment, PacketReceipt};

const IBC_STORE_PATH: &str = "store/ibc/key";

const COMMITMENT_DISCRIMINATOR: u8 = 0x01;
const RECEIPT_DISCRIMINATOR: u8 = 0x02;
const ACK_DISCRIMINATOR: u8 = 0x03;

/// ABCI state at height H is committed in block `H+1`'s `app_hash`.
/// When the light client is updated to height H, proofs must be
/// queried at `H-1` to match the `app_hash` the client holds.
fn proof_query_height(height: TmHeight) -> Result<TmHeight> {
    let prev = height
        .value()
        .checked_sub(1)
        .ok_or_else(|| eyre::eyre!("proof height underflow: height is 0"))?;
    let h = TmHeight::try_from(prev)
        .map_err(|e| eyre::eyre!("invalid proof query height {prev}: {e}"))?;
    Ok(h)
}

/// IBC v2 key: `client_bytes` || `discriminator` || `sequence_be_bytes`
fn ibc_v2_key(client: &str, discriminator: u8, sequence: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(client.len() + 1 + 8);
    key.extend_from_slice(client.as_bytes());
    key.push(discriminator);
    key.extend_from_slice(&sequence.to_be_bytes());
    key
}

fn extract_proof(
    response: &tendermint_rpc::endpoint::abci_query::AbciQuery,
) -> Result<MerkleProof> {
    let proof_ops = response
        .proof
        .as_ref()
        .ok_or_else(|| eyre::eyre!("missing proof in ABCI query response"))?;

    let proofs: Vec<ibc_proto::ics23::CommitmentProof> = proof_ops
        .ops
        .iter()
        .filter_map(
            |op| match ibc_proto::ics23::CommitmentProof::decode(op.data.as_slice()) {
                Ok(proof) => Some(proof),
                Err(e) => {
                    warn!("failed to decode CommitmentProof op: {e}");
                    None
                }
            },
        )
        .collect();

    let merkle_proof = ibc_proto::ibc::core::commitment::v1::MerkleProof { proofs };
    Ok(MerkleProof {
        proof_bytes: merkle_proof.encode_to_vec(),
    })
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
        let query_height = proof_query_height(*height)?;
        let response = query_abci(
            &self.rpc_client,
            IBC_STORE_PATH,
            ibc_v2_key(client_id.as_str(), COMMITMENT_DISCRIMINATOR, sequence),
            Some(query_height),
            true,
        )
        .await?;

        let proof = extract_proof(&response)?;
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
        let query_height = proof_query_height(*height)?;
        let response = query_abci(
            &self.rpc_client,
            IBC_STORE_PATH,
            ibc_v2_key(client_id.as_str(), RECEIPT_DISCRIMINATOR, sequence),
            Some(query_height),
            true,
        )
        .await?;

        let proof = extract_proof(&response)?;
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
        let query_height = proof_query_height(*height)?;
        let response = query_abci(
            &self.rpc_client,
            IBC_STORE_PATH,
            ibc_v2_key(client_id.as_str(), ACK_DISCRIMINATOR, sequence),
            Some(query_height),
            true,
        )
        .await?;

        let proof = extract_proof(&response)?;
        let ack = if response.value.is_empty() {
            None
        } else {
            Some(PacketAcknowledgement(response.value))
        };
        Ok((ack, proof))
    }
}
