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

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "mock attestor handlers return tonic::Status"
)]
mod tests {
    use super::*;
    use crate::test_support::{MockAttestor, dead_endpoint, wire};
    use tonic::Status;

    const TIMEOUT: Duration = Duration::from_secs(5);

    fn client(endpoint: &str) -> AttestorClient {
        AttestorClient::new(endpoint, TIMEOUT).unwrap()
    }

    fn wire_att(height: u64) -> wire::Attestation {
        wire::Attestation {
            height,
            timestamp: Some(1_700_000_000),
            attested_data: vec![0xaa, 0xbb],
            signature: vec![7; 65],
        }
    }

    #[test]
    fn commitment_type_discriminants_match_attestor_proto() {
        // COMMITMENT_TYPE_{PACKET,ACK,RECEIPT} in ibc_attestor.proto.
        assert_eq!(CommitmentType::Packet as i32, 0);
        assert_eq!(CommitmentType::Ack as i32, 1);
        assert_eq!(CommitmentType::Receipt as i32, 2);
    }

    #[tokio::test]
    async fn rejects_unparseable_endpoint() {
        let err = AttestorClient::new("not a uri", TIMEOUT).unwrap_err();
        assert!(
            err.to_string()
                .contains("invalid attestor endpoint not a uri")
        );
    }

    #[tokio::test]
    async fn construction_does_not_contact_the_attestor() {
        let endpoint = dead_endpoint().await;
        let client = AttestorClient::new(&endpoint, TIMEOUT).unwrap();
        assert_eq!(client.endpoint(), endpoint);
    }

    #[tokio::test]
    async fn latest_height_returns_server_height() {
        let endpoint = MockAttestor::default().height(|| Ok(4242)).spawn().await;
        assert_eq!(client(&endpoint).latest_height().await.unwrap(), 4242);
    }

    #[tokio::test]
    async fn state_attestation_requests_height_and_decodes_every_field() {
        let mock = MockAttestor::default().state(|h| Ok(Some(wire_att(h))));
        let requests = std::sync::Arc::clone(&mock.state_requests);
        let endpoint = mock.spawn().await;

        let att = client(&endpoint).state_attestation(77).await.unwrap();

        assert_eq!(
            att,
            Attestation {
                height: 77,
                timestamp: Some(1_700_000_000),
                attested_data: vec![0xaa, 0xbb],
                signature: vec![7; 65],
            }
        );
        assert_eq!(
            *requests.lock().unwrap(),
            vec![wire::StateAttestationRequest { height: 77 }]
        );
    }

    #[tokio::test]
    async fn absent_timestamp_decodes_as_none() {
        let endpoint = MockAttestor::default()
            .state(|h| {
                Ok(Some(wire::Attestation {
                    timestamp: None,
                    ..wire_att(h)
                }))
            })
            .spawn()
            .await;
        let att = client(&endpoint).state_attestation(1).await.unwrap();
        assert_eq!(att.timestamp, None);
    }

    #[tokio::test]
    async fn packet_attestation_sends_packets_height_and_commitment_type() {
        let mock = MockAttestor::default().packet(|req| Ok(Some(wire_att(req.height))));
        let requests = std::sync::Arc::clone(&mock.packet_requests);
        let c = client(&mock.spawn().await);

        let packets = vec![b"p1".to_vec(), Vec::new(), b"p3".to_vec()];
        for ct in [
            CommitmentType::Packet,
            CommitmentType::Ack,
            CommitmentType::Receipt,
        ] {
            let att = c.packet_attestation(packets.clone(), 9, ct).await.unwrap();
            assert_eq!(att.height, 9);
        }

        let seen: Vec<_> = requests.lock().unwrap().clone();
        assert_eq!(
            seen.iter().map(|r| r.commitment_type).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(seen.iter().all(|r| r.height == 9 && r.packets == packets));
    }

    #[tokio::test]
    async fn missing_state_attestation_is_an_error() {
        let endpoint = MockAttestor::default().state(|_| Ok(None)).spawn().await;
        let err = client(&endpoint).state_attestation(1).await.unwrap_err();
        assert!(err.to_string().contains("returned no state attestation"));
        assert!(err.to_string().contains(&endpoint));
    }

    #[tokio::test]
    async fn missing_packet_attestation_is_an_error() {
        let endpoint = MockAttestor::default().packet(|_| Ok(None)).spawn().await;
        let err = client(&endpoint)
            .packet_attestation(vec![b"p".to_vec()], 1, CommitmentType::Ack)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("returned no packet attestation"));
    }

    #[tokio::test]
    async fn server_status_is_reported_with_path_message_and_code() {
        let endpoint = MockAttestor::default()
            .height(|| Err(Status::failed_precondition("not finalized")))
            .spawn()
            .await;
        let msg = client(&endpoint)
            .latest_height()
            .await
            .unwrap_err()
            .to_string();
        assert!(msg.contains(&endpoint), "{msg}");
        assert!(
            msg.contains("/ibc_attestor.AttestationService/LatestHeight"),
            "{msg}"
        );
        assert!(msg.contains("not finalized"), "{msg}");
        assert!(msg.contains("FailedPrecondition"), "{msg}");
    }

    #[tokio::test]
    async fn unreachable_attestor_errors() {
        let endpoint = dead_endpoint().await;
        let err = client(&endpoint).latest_height().await.unwrap_err();
        assert!(err.to_string().contains(&endpoint), "{err}");
    }

    #[tokio::test]
    async fn undecodable_response_errors() {
        let endpoint = MockAttestor::default()
            .garbage_latest_height()
            .spawn()
            .await;
        let err = client(&endpoint).latest_height().await.unwrap_err();
        assert!(err.to_string().contains("LatestHeight failed"), "{err}");
    }

    #[tokio::test]
    async fn non_grpc_peer_errors() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            while let Ok((mut sock, _)) = listener.accept().await {
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\n\r\nhello").await;
            }
        });
        let err = client(&format!("http://{addr}"))
            .state_attestation(1)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("attestor"), "{err}");
    }

    #[tokio::test]
    async fn slow_attestor_hits_request_timeout() {
        let endpoint = MockAttestor::default()
            .height(|| Ok(1))
            .delay(Duration::from_secs(30))
            .spawn()
            .await;
        let c = AttestorClient::new(&endpoint, Duration::from_millis(200)).unwrap();
        let started = std::time::Instant::now();
        assert!(c.latest_height().await.is_err());
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
