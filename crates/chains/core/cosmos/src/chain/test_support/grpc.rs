//! In-process gRPC mock. A single catch-all service answers every method path
//! with raw protobuf bytes, so tests can stand in for any Cosmos SDK query
//! service (including ones without generated server traits, like osmosis
//! txfees or skip feemarket). Unhandled paths should return `unimplemented`,
//! mirroring a node that does not register that service.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use prost::bytes::{Buf, BufMut};
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::codegen::{BoxFuture, Service, http};
use tonic::server::UnaryService;

#[derive(Clone, Debug)]
pub struct GrpcCall {
    pub path: String,
    /// Value of the `x-cosmos-block-height` metadata header, if sent.
    pub height: Option<String>,
    pub body: Vec<u8>,
}

impl GrpcCall {
    pub fn decode<M: prost::Message + Default>(&self) -> M {
        M::decode(self.body.as_slice()).unwrap()
    }
}

pub enum GrpcReply {
    Ok(Vec<u8>),
    Err(tonic::Status),
    Hang,
}

pub fn reply<M: prost::Message>(msg: &M) -> GrpcReply {
    GrpcReply::Ok(msg.encode_to_vec())
}

pub fn unimplemented(call: &GrpcCall) -> GrpcReply {
    GrpcReply::Err(tonic::Status::unimplemented(format!(
        "unknown service {}",
        call.path
    )))
}

type Handler = dyn Fn(&GrpcCall) -> GrpcReply + Send + Sync;

pub struct MockGrpc {
    pub url: String,
    calls: Arc<Mutex<Vec<GrpcCall>>>,
}

impl MockGrpc {
    pub fn calls(&self) -> Vec<GrpcCall> {
        self.calls.lock().unwrap().clone()
    }

    pub fn calls_to(&self, path: &str) -> Vec<GrpcCall> {
        self.calls()
            .into_iter()
            .filter(|c| c.path == path)
            .collect()
    }
}

pub async fn spawn_grpc<F>(handler: F) -> MockGrpc
where
    F: Fn(&GrpcCall) -> GrpcReply + Send + Sync + 'static,
{
    let incoming =
        tonic::transport::server::TcpIncoming::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let url = format!("http://{}", incoming.local_addr().unwrap());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let svc = CatchAll {
        handler: Arc::new(handler),
        calls: Arc::clone(&calls),
    };
    tokio::spawn(tonic::transport::Server::builder().serve_with_incoming(svc, incoming));
    MockGrpc { url, calls }
}

#[derive(Clone)]
struct CatchAll {
    handler: Arc<Handler>,
    calls: Arc<Mutex<Vec<GrpcCall>>>,
}

impl Service<http::Request<tonic::body::Body>> for CatchAll {
    type Response = http::Response<tonic::body::Body>;
    type Error = Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<tonic::body::Body>) -> Self::Future {
        let unary = Unary {
            path: req.uri().path().to_string(),
            svc: self.clone(),
        };
        Box::pin(async move { Ok(tonic::server::Grpc::new(RawCodec).unary(unary, req).await) })
    }
}

struct Unary {
    path: String,
    svc: CatchAll,
}

impl UnaryService<Vec<u8>> for Unary {
    type Response = Vec<u8>;
    type Future = BoxFuture<tonic::Response<Vec<u8>>, tonic::Status>;

    fn call(&mut self, request: tonic::Request<Vec<u8>>) -> Self::Future {
        let height = request
            .metadata()
            .get("x-cosmos-block-height")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let call = GrpcCall {
            path: self.path.clone(),
            height,
            body: request.into_inner(),
        };
        let reply = (self.svc.handler)(&call);
        self.svc.calls.lock().unwrap().push(call);
        Box::pin(async move {
            match reply {
                GrpcReply::Ok(bytes) => Ok(tonic::Response::new(bytes)),
                GrpcReply::Err(status) => Err(status),
                GrpcReply::Hang => std::future::pending().await,
            }
        })
    }
}

#[derive(Clone, Copy, Default)]
struct RawCodec;

impl Codec for RawCodec {
    type Encode = Vec<u8>;
    type Decode = Vec<u8>;
    type Encoder = Self;
    type Decoder = Self;

    fn encoder(&mut self) -> Self {
        Self
    }

    fn decoder(&mut self) -> Self {
        Self
    }
}

impl Encoder for RawCodec {
    type Item = Vec<u8>;
    type Error = tonic::Status;

    fn encode(&mut self, item: Vec<u8>, dst: &mut EncodeBuf<'_>) -> Result<(), tonic::Status> {
        dst.put_slice(&item);
        Ok(())
    }
}

impl Decoder for RawCodec {
    type Item = Vec<u8>;
    type Error = tonic::Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Vec<u8>>, tonic::Status> {
        let mut out = vec![0u8; src.remaining()];
        src.copy_to_slice(&mut out);
        Ok(Some(out))
    }
}
