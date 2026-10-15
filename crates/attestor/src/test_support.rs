// In-process gRPC attestor used by the client and aggregator tests.
//
// The `wire` messages are transcribed from
// external/solidity-ibc-eureka/proto/ibc_attestor/*.proto rather than reusing
// `crate::proto`, so a tag or type mistake in the production structs shows up
// as a decode mismatch instead of round-tripping silently.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tonic::body::Body;
use tonic::codec::ProstCodec;
use tonic::codegen::{BoxFuture, Context, Poll, Service, http};
use tonic::server::{Grpc, NamedService, UnaryService};
use tonic::transport::server::TcpIncoming;
use tonic::{Request, Response, Status};

pub mod wire {
    use prost::Message;

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct Attestation {
        #[prost(uint64, tag = "1")]
        pub height: u64,
        #[prost(uint64, optional, tag = "2")]
        pub timestamp: Option<u64>,
        #[prost(bytes = "vec", tag = "3")]
        pub attested_data: Vec<u8>,
        #[prost(bytes = "vec", tag = "4")]
        pub signature: Vec<u8>,
    }

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct StateAttestationRequest {
        #[prost(uint64, tag = "1")]
        pub height: u64,
    }

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct AttestationResponse {
        #[prost(message, optional, tag = "1")]
        pub attestation: Option<Attestation>,
    }

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct PacketAttestationRequest {
        #[prost(bytes = "vec", repeated, tag = "1")]
        pub packets: Vec<Vec<u8>>,
        #[prost(uint64, tag = "2")]
        pub height: u64,
        #[prost(int32, tag = "3")]
        pub commitment_type: i32,
    }

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct LatestHeightRequest {}

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct LatestHeightResponse {
        #[prost(uint64, tag = "1")]
        pub height: u64,
    }

    // Field 1 as a string: wire type LEN where clients expect a varint.
    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct Garbage {
        #[prost(string, tag = "1")]
        pub junk: String,
    }
}

pub const SERVICE: &str = "ibc_attestor.AttestationService";

type Handler<Req, Resp> = Arc<dyn Fn(Req) -> Result<Resp, Status> + Send + Sync>;

#[derive(Clone)]
pub struct MockAttestor {
    latest_height: Handler<wire::LatestHeightRequest, wire::LatestHeightResponse>,
    state: Handler<wire::StateAttestationRequest, wire::AttestationResponse>,
    packet: Handler<wire::PacketAttestationRequest, wire::AttestationResponse>,
    garbage_latest_height: bool,
    delay: Duration,
    pub state_requests: Arc<Mutex<Vec<wire::StateAttestationRequest>>>,
    pub packet_requests: Arc<Mutex<Vec<wire::PacketAttestationRequest>>>,
}

impl Default for MockAttestor {
    fn default() -> Self {
        Self {
            latest_height: Arc::new(|_| Err(Status::unimplemented("latest height"))),
            state: Arc::new(|_| Err(Status::unimplemented("state"))),
            packet: Arc::new(|_| Err(Status::unimplemented("packet"))),
            garbage_latest_height: false,
            delay: Duration::ZERO,
            state_requests: Arc::default(),
            packet_requests: Arc::default(),
        }
    }
}

impl MockAttestor {
    pub fn height(mut self, f: impl Fn() -> Result<u64, Status> + Send + Sync + 'static) -> Self {
        self.latest_height =
            Arc::new(move |_| f().map(|height| wire::LatestHeightResponse { height }));
        self
    }

    pub fn state(
        mut self,
        f: impl Fn(u64) -> Result<Option<wire::Attestation>, Status> + Send + Sync + 'static,
    ) -> Self {
        let log = Arc::clone(&self.state_requests);
        self.state = Arc::new(move |req| {
            log.lock().unwrap().push(req.clone());
            f(req.height).map(|attestation| wire::AttestationResponse { attestation })
        });
        self
    }

    pub fn packet(
        mut self,
        f: impl Fn(&wire::PacketAttestationRequest) -> Result<Option<wire::Attestation>, Status>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        let log = Arc::clone(&self.packet_requests);
        self.packet = Arc::new(move |req| {
            log.lock().unwrap().push(req.clone());
            f(&req).map(|attestation| wire::AttestationResponse { attestation })
        });
        self
    }

    pub const fn garbage_latest_height(mut self) -> Self {
        self.garbage_latest_height = true;
        self
    }

    pub const fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub async fn spawn(self) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(self)
                .serve_with_incoming(TcpIncoming::from(listener)),
        );
        format!("http://{addr}")
    }
}

impl NamedService for MockAttestor {
    const NAME: &'static str = SERVICE;
}

impl Service<http::Request<Body>> for MockAttestor {
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<Body>) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            tokio::time::sleep(this.delay).await;
            let method = req.uri().path().rsplit('/').next().unwrap_or_default();
            Ok(match method {
                "LatestHeight" if this.garbage_latest_height => {
                    let junk = Arc::new(|_: wire::LatestHeightRequest| {
                        Ok(wire::Garbage {
                            junk: "not a height".to_string(),
                        })
                    });
                    serve(junk, req).await
                }
                "LatestHeight" => serve(this.latest_height, req).await,
                "StateAttestation" => serve(this.state, req).await,
                "PacketAttestation" => serve(this.packet, req).await,
                other => Status::unimplemented(other.to_string()).into_http(),
            })
        })
    }
}

struct Unary<Req, Resp>(Handler<Req, Resp>);

impl<Req, Resp> UnaryService<Req> for Unary<Req, Resp>
where
    Req: Send + 'static,
    Resp: Send + 'static,
{
    type Response = Resp;
    type Future = BoxFuture<Response<Resp>, Status>;

    fn call(&mut self, request: Request<Req>) -> Self::Future {
        let handler = Arc::clone(&self.0);
        Box::pin(async move { handler(request.into_inner()).map(Response::new) })
    }
}

async fn serve<Req, Resp>(
    handler: Handler<Req, Resp>,
    req: http::Request<Body>,
) -> http::Response<Body>
where
    Req: prost::Message + Default + Send + 'static,
    Resp: prost::Message + Send + 'static,
{
    Grpc::new(ProstCodec::<Resp, Req>::default())
        .unary(Unary(handler), req)
        .await
}

// An endpoint nothing listens on: bind, note the port, release it.
pub async fn dead_endpoint() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}
