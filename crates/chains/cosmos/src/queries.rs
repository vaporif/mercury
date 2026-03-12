use async_trait::async_trait;
use ibc_client_tendermint::types::ClientState as TendermintClientState;
use ibc_client_tendermint::types::ConsensusState as TendermintConsensusState;
use ibc_proto::Protobuf;
use ibc_proto::ibc::core::client::v1::{
    QueryClientStateRequest, QueryClientStateResponse, QueryConsensusStateRequest,
    QueryConsensusStateResponse,
};
use prost::Message;
use tendermint_rpc::Client;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};

use mercury_chain_traits::queries::{
    CanQueryChainStatus, CanQueryClientState, CanQueryConsensusState, HasClientLatestHeight,
    HasTrustingPeriod,
};
use mercury_chain_traits::types::HasChainTypes;
use mercury_core::error::{Error, Result};

use crate::chain::CosmosChain;
use crate::keys::CosmosSigner;
use crate::types::CosmosChainStatus;

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

#[async_trait]
impl<S: CosmosSigner> CanQueryChainStatus for CosmosChain<S> {
    async fn query_chain_status(&self) -> Result<Self::ChainStatus> {
        let status = self.rpc_client.status().await.map_err(Error::report)?;
        Ok(CosmosChainStatus {
            height: status.sync_info.latest_block_height,
            timestamp: status.sync_info.latest_block_time,
        })
    }
}

#[async_trait]
impl<S: CosmosSigner> CanQueryClientState<Self> for CosmosChain<S> {
    async fn query_client_state(
        &self,
        client_id: &Self::ClientId,
        height: &Self::Height,
    ) -> Result<Self::ClientState> {
        let mut request = tonic::Request::new(QueryClientStateRequest {
            client_id: client_id.to_string(),
        });

        request.metadata_mut().insert(
            "x-cosmos-block-height",
            height.value().to_string().parse().map_err(Error::report)?,
        );

        let response = grpc_unary::<QueryClientStateRequest, QueryClientStateResponse>(
            self.grpc_channel.clone(),
            "/ibc.core.client.v1.Query/ClientState",
            request,
        )
        .await
        .map_err(Error::report)?
        .into_inner();

        let any = response
            .client_state
            .ok_or_else(|| Error::report(eyre::eyre!("client state not found for {client_id}")))?;

        let client_state = <TendermintClientState as Protobuf<
            ibc_client_tendermint::types::proto::v1::ClientState,
        >>::decode(any.value.as_slice())
        .map_err(Error::report)?;
        Ok(client_state)
    }
}

#[async_trait]
impl<S: CosmosSigner> CanQueryConsensusState<Self> for CosmosChain<S> {
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
            query_height
                .value()
                .to_string()
                .parse()
                .map_err(Error::report)?,
        );

        let response = grpc_unary::<QueryConsensusStateRequest, QueryConsensusStateResponse>(
            self.grpc_channel.clone(),
            "/ibc.core.client.v1.Query/ConsensusState",
            request,
        )
        .await
        .map_err(Error::report)?
        .into_inner();

        let any = response.consensus_state.ok_or_else(|| {
            Error::report(eyre::eyre!(
                "consensus state not found for {client_id} at height {consensus_height}"
            ))
        })?;

        let consensus_state = <TendermintConsensusState as Protobuf<
            ibc_client_tendermint::types::proto::v1::ConsensusState,
        >>::decode(any.value.as_slice())
        .map_err(Error::report)?;
        Ok(consensus_state)
    }
}

impl<S: CosmosSigner> HasTrustingPeriod<Self> for CosmosChain<S> {
    fn trusting_period(client_state: &Self::ClientState) -> Option<std::time::Duration> {
        Some(client_state.trusting_period)
    }
}

impl<S: CosmosSigner> HasClientLatestHeight<Self> for CosmosChain<S> {
    fn client_latest_height(client_state: &Self::ClientState) -> Self::Height {
        let h = client_state.latest_height.revision_height();
        tendermint::block::Height::try_from(h.max(1)).unwrap_or_else(|_| tendermint::block::Height::from(1_u32))
    }
}
