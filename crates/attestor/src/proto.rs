//! Hand-written prost types for the attestor gRPC API (`ibc_attestor` package)
//! and ibc-go's `ibc.lightclients.attestations.v1` light client.

use prost::Message;

pub mod attestor {
    use super::Message;

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
    pub struct StateAttestationResponse {
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
    pub struct PacketAttestationResponse {
        #[prost(message, optional, tag = "1")]
        pub attestation: Option<Attestation>,
    }

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct LatestHeightRequest {}

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct LatestHeightResponse {
        #[prost(uint64, tag = "1")]
        pub height: u64,
    }
}

pub mod lightclient {
    use super::Message;

    pub const CLIENT_STATE_TYPE_URL: &str = "/ibc.lightclients.attestations.v1.ClientState";
    pub const CONSENSUS_STATE_TYPE_URL: &str = "/ibc.lightclients.attestations.v1.ConsensusState";
    pub const ATTESTATION_PROOF_TYPE_URL: &str =
        "/ibc.lightclients.attestations.v1.AttestationProof";

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct ClientState {
        #[prost(string, repeated, tag = "1")]
        pub attestor_addresses: Vec<String>,
        #[prost(uint32, tag = "2")]
        pub min_required_sigs: u32,
        #[prost(uint64, tag = "3")]
        pub latest_height: u64,
        #[prost(bool, tag = "4")]
        pub is_frozen: bool,
    }

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct ConsensusState {
        /// Nanoseconds since the Unix epoch.
        #[prost(uint64, tag = "1")]
        pub timestamp: u64,
    }

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct AttestationProof {
        #[prost(bytes = "vec", tag = "1")]
        pub attestation_data: Vec<u8>,
        #[prost(bytes = "vec", repeated, tag = "2")]
        pub signatures: Vec<Vec<u8>>,
    }
}
