use std::time::Duration;

use eyre::{Context, Result};
use tonic::client::Grpc;
use tonic::codec::ProstCodec;
use tonic::codegen::http::uri::PathAndQuery;
use tonic::transport::{Channel, Endpoint};

use crate::proto::attestor::{
    Attestation, LatestHeightRequest, LatestHeightResponse, PacketAttestationRequest,
    PacketAttestationResponse, StateAttestationRequest, StateAttestationResponse,
};

/// What a packet attestation proves about the packet on the attested chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommitmentType {
    /// The packet commitment exists and matches the packet (recv).
    Packet = 0,
    /// The acknowledgement commitment exists (ack).
    Ack = 1,
    /// No receipt exists for the packet (timeout).
    Receipt = 2,
}

/// gRPC client for a single `ibc_attestor.AttestationService` endpoint.
#[derive(Clone, Debug)]
pub struct AttestorClient {
    endpoint: String,
    grpc: Grpc<Channel>,
}

impl AttestorClient {
    /// Connects lazily, so construction never blocks on the attestor being up.
    pub fn new(endpoint: &str, timeout: Duration) -> Result<Self> {
        let channel = Endpoint::from_shared(endpoint.to_string())
            .wrap_err_with(|| format!("invalid attestor endpoint {endpoint}"))?
            .timeout(timeout)
            .connect_timeout(timeout)
            .connect_lazy();
        Ok(Self {
            endpoint: endpoint.to_string(),
            grpc: Grpc::new(channel),
        })
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn latest_height(&self) -> Result<u64> {
        let resp: LatestHeightResponse = self
            .unary(
                "/ibc_attestor.AttestationService/LatestHeight",
                LatestHeightRequest {},
            )
            .await?;
        Ok(resp.height)
    }

    pub async fn state_attestation(&self, height: u64) -> Result<Attestation> {
        let resp: StateAttestationResponse = self
            .unary(
                "/ibc_attestor.AttestationService/StateAttestation",
                StateAttestationRequest { height },
            )
            .await?;
        resp.attestation
            .ok_or_else(|| eyre::eyre!("attestor {} returned no state attestation", self.endpoint))
    }

    pub async fn packet_attestation(
        &self,
        packets: Vec<Vec<u8>>,
        height: u64,
        commitment_type: CommitmentType,
    ) -> Result<Attestation> {
        let resp: PacketAttestationResponse = self
            .unary(
                "/ibc_attestor.AttestationService/PacketAttestation",
                PacketAttestationRequest {
                    packets,
                    height,
                    commitment_type: commitment_type as i32,
                },
            )
            .await?;
        resp.attestation
            .ok_or_else(|| eyre::eyre!("attestor {} returned no packet attestation", self.endpoint))
    }

    async fn unary<Req, Resp>(&self, path: &'static str, request: Req) -> Result<Resp>
    where
        Req: prost::Message + Send + Sync + 'static,
        Resp: prost::Message + Default + Send + Sync + 'static,
    {
        let mut grpc = self.grpc.clone();
        grpc.ready()
            .await
            .wrap_err_with(|| format!("attestor {} not ready", self.endpoint))?;
        let resp = grpc
            .unary(
                tonic::Request::new(request),
                PathAndQuery::from_static(path),
                ProstCodec::default(),
            )
            .await
            .map_err(|status| {
                eyre::eyre!(
                    "attestor {} {path} failed: {} ({:?})",
                    self.endpoint,
                    status.message(),
                    status.code()
                )
            })?;
        Ok(resp.into_inner())
    }
}
