use async_trait::async_trait;
use ibc_client_tendermint::types::ClientState as TendermintClientState;
use ibc_client_tendermint::types::ConsensusState as TendermintConsensusState;
use ibc_proto::Protobuf;
use ibc_proto::ibc::core::client::v1::{
    QueryClientStateRequest, QueryClientStateResponse, QueryConsensusStateRequest,
    QueryConsensusStateResponse,
};
use prost::Message;
use tendermint::block::Height as TmHeight;
use tendermint_rpc::{Client, HttpClient};
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tracing::{instrument, warn};

use mercury_chain_traits::queries::{CanQueryChainStatus, CanQueryClient, CanQueryPacketState};
use mercury_chain_traits::types::HasChainTypes;
use mercury_core::error::Result;

use crate::chain::CosmosChain;
use crate::keys::CosmosSigner;
use crate::types::{
    CosmosChainStatus, MerkleProof, PacketAcknowledgement, PacketCommitment, PacketReceipt,
};

/// A codec that encodes/decodes protobuf messages using prost's `Message`
/// trait through tonic's codec interface.
#[derive(Debug, Clone)]
pub(crate) struct ProstMessageCodec<T, U>(std::marker::PhantomData<(T, U)>);

impl<T, U> Default for ProstMessageCodec<T, U> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProstMessageEncoder<T>(std::marker::PhantomData<T>);

#[derive(Debug, Clone)]
pub(crate) struct ProstMessageDecoder<U>(std::marker::PhantomData<U>);

impl<T: Message + Send + 'static> Encoder for ProstMessageEncoder<T> {
    type Item = T;
    type Error = tonic::Status;

    fn encode(
        &mut self,
        item: Self::Item,
        dst: &mut EncodeBuf<'_>,
    ) -> core::result::Result<(), Self::Error> {
        item.encode(dst)
            .map_err(|e| tonic::Status::internal(format!("encode error: {e}")))
    }
}

impl<U: Message + Default + Send + 'static> Decoder for ProstMessageDecoder<U> {
    type Item = U;
    type Error = tonic::Status;

    fn decode(
        &mut self,
        src: &mut DecodeBuf<'_>,
    ) -> core::result::Result<Option<Self::Item>, Self::Error> {
        let item =
            U::decode(src).map_err(|e| tonic::Status::internal(format!("decode error: {e}")))?;
        Ok(Some(item))
    }
}

impl<T, U> Codec for ProstMessageCodec<T, U>
where
    T: Message + Send + 'static,
    U: Message + Default + Send + 'static,
{
    type Encode = T;
    type Decode = U;
    type Encoder = ProstMessageEncoder<T>;
    type Decoder = ProstMessageDecoder<U>;

    fn encoder(&mut self) -> Self::Encoder {
        ProstMessageEncoder(std::marker::PhantomData)
    }

    fn decoder(&mut self) -> Self::Decoder {
        ProstMessageDecoder(std::marker::PhantomData)
    }
}

/// Makes a unary gRPC call using prost message types via tonic.
pub(crate) async fn grpc_unary<Req, Resp>(
    channel: tonic::transport::Channel,
    path: &str,
    request: tonic::Request<Req>,
) -> core::result::Result<tonic::Response<Resp>, tonic::Status>
where
    Req: Message + Send + Sync + 'static,
    Resp: Message + Default + Send + Sync + 'static,
{
    let mut client = tonic::client::Grpc::new(channel);
    client
        .ready()
        .await
        .map_err(|e| tonic::Status::unknown(format!("service not ready: {e}")))?;
    let path: tonic::codegen::http::uri::PathAndQuery = path
        .parse()
        .map_err(|e| tonic::Status::internal(format!("invalid path: {e}")))?;
    let codec = ProstMessageCodec::<Req, Resp>::default();
    client.unary(request, path, codec).await
}

pub(crate) async fn query_abci(
    client: &HttpClient,
    path: &str,
    data: Vec<u8>,
    height: Option<TmHeight>,
    prove: bool,
) -> Result<tendermint_rpc::endpoint::abci_query::AbciQuery> {
    let response = client
        .abci_query(Some(path.to_string()), data, height, prove)
        .await?;
    Ok(response)
}

/// Query chain status via RPC only. No keys, no gRPC — lightweight health check.
pub async fn query_cosmos_status(rpc_addr: &str) -> Result<CosmosChainStatus> {
    let client = HttpClient::new(rpc_addr)?;
    let status = client.status().await?;
    Ok(CosmosChainStatus {
        height: status.sync_info.latest_block_height,
        timestamp: status.sync_info.latest_block_time,
    })
}

#[async_trait]
impl<S: CosmosSigner> CanQueryChainStatus for CosmosChain<S> {
    #[instrument(skip_all, name = "query_chain_status")]
    async fn query_chain_status(&self) -> Result<Self::ChainStatus> {
        let status = self.rpc_client.status().await?;
        Ok(CosmosChainStatus {
            height: status.sync_info.latest_block_height,
            timestamp: status.sync_info.latest_block_time,
        })
    }
}

#[async_trait]
impl<S: CosmosSigner> CanQueryClient<Self> for CosmosChain<S> {
    #[instrument(skip_all, name = "query_client_state", fields(client_id = %client_id))]
    async fn query_client_state(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Self::ClientState> {
        let mut request = tonic::Request::new(QueryClientStateRequest {
            client_id: client_id.to_string(),
        });

        request
            .metadata_mut()
            .insert("x-cosmos-block-height", height.value().to_string().parse()?);

        let response = grpc_unary::<QueryClientStateRequest, QueryClientStateResponse>(
            self.grpc_channel.clone(),
            "/ibc.core.client.v1.Query/ClientState",
            request,
        )
        .await?
        .into_inner();

        let any = response
            .client_state
            .ok_or_else(|| eyre::eyre!("client state not found for {client_id}"))?;

        let client_state = <TendermintClientState as Protobuf<
            ibc_client_tendermint::types::proto::v1::ClientState,
        >>::decode(any.value.as_slice())?;
        Ok(client_state)
    }

    #[instrument(skip_all, name = "query_consensus_state", fields(client_id = %client_id))]
    async fn query_consensus_state(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &<Self as HasChainTypes>::Height,
        query_height: &Self::Height,
    ) -> Result<Self::ConsensusState> {
        let revision_height = consensus_height.value();
        let revision_number = self.chain_id.revision_number();

        let mut request = tonic::Request::new(QueryConsensusStateRequest {
            client_id: client_id.to_string(),
            revision_number,
            revision_height,
            latest_height: false,
        });

        request.metadata_mut().insert(
            "x-cosmos-block-height",
            query_height.value().to_string().parse()?,
        );

        let response = grpc_unary::<QueryConsensusStateRequest, QueryConsensusStateResponse>(
            self.grpc_channel.clone(),
            "/ibc.core.client.v1.Query/ConsensusState",
            request,
        )
        .await?
        .into_inner();

        let any = response.consensus_state.ok_or_else(|| {
            eyre::eyre!("consensus state not found for {client_id} at height {consensus_height}")
        })?;

        let consensus_state = <TendermintConsensusState as Protobuf<
            ibc_client_tendermint::types::proto::v1::ConsensusState,
        >>::decode(any.value.as_slice())?;
        Ok(consensus_state)
    }

    fn trusting_period(client_state: &Self::ClientState) -> Option<std::time::Duration> {
        Some(client_state.trusting_period)
    }

    fn client_latest_height(client_state: &Self::ClientState) -> Self::Height {
        let h = client_state.latest_height.revision_height();
        TmHeight::try_from(h.max(1))
            .unwrap_or_else(|_| TmHeight::from(1_u32))
    }
}

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
impl<S: CosmosSigner> CanQueryPacketState<Self> for CosmosChain<S> {
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
