//! IBC v2 proto types from canonical definitions in `cosmos/ibc-go`.
//!
//! Not yet available in `ibc-proto` 0.52, so defined manually with matching
//! field numbers and type URLs.

/// Channel v2 types from `ibc.core.channel.v2`.
pub mod channel {
    use prost::Message;

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct Packet {
        #[prost(uint64, tag = "1")]
        pub sequence: u64,
        #[prost(string, tag = "2")]
        pub source_client: String,
        #[prost(string, tag = "3")]
        pub destination_client: String,
        #[prost(uint64, tag = "4")]
        pub timeout_timestamp: u64,
        #[prost(message, repeated, tag = "5")]
        pub payloads: Vec<Payload>,
    }

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct Payload {
        #[prost(string, tag = "1")]
        pub source_port: String,
        #[prost(string, tag = "2")]
        pub destination_port: String,
        #[prost(string, tag = "3")]
        pub version: String,
        #[prost(string, tag = "4")]
        pub encoding: String,
        #[prost(bytes = "vec", tag = "5")]
        pub value: Vec<u8>,
    }

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct Acknowledgement {
        #[prost(bytes = "vec", repeated, tag = "1")]
        pub app_acknowledgements: Vec<Vec<u8>>,
    }

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct MsgRecvPacket {
        #[prost(message, optional, tag = "1")]
        pub packet: Option<Packet>,
        #[prost(bytes = "vec", tag = "2")]
        pub proof_commitment: Vec<u8>,
        #[prost(message, optional, tag = "3")]
        pub proof_height: Option<ibc_proto::ibc::core::client::v1::Height>,
        #[prost(string, tag = "4")]
        pub signer: String,
    }

    impl prost::Name for MsgRecvPacket {
        const NAME: &'static str = "MsgRecvPacket";
        const PACKAGE: &'static str = "ibc.core.channel.v2";
        fn full_name() -> String {
            "ibc.core.channel.v2.MsgRecvPacket".into()
        }
        fn type_url() -> String {
            "/ibc.core.channel.v2.MsgRecvPacket".into()
        }
    }

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct MsgTimeout {
        #[prost(message, optional, tag = "1")]
        pub packet: Option<Packet>,
        #[prost(bytes = "vec", tag = "2")]
        pub proof_unreceived: Vec<u8>,
        #[prost(message, optional, tag = "3")]
        pub proof_height: Option<ibc_proto::ibc::core::client::v1::Height>,
        // Tag 5 matches canonical proto (tag 4 is skipped).
        #[prost(string, tag = "5")]
        pub signer: String,
    }

    impl prost::Name for MsgTimeout {
        const NAME: &'static str = "MsgTimeout";
        const PACKAGE: &'static str = "ibc.core.channel.v2";
        fn full_name() -> String {
            "ibc.core.channel.v2.MsgTimeout".into()
        }
        fn type_url() -> String {
            "/ibc.core.channel.v2.MsgTimeout".into()
        }
    }

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct MsgAcknowledgement {
        #[prost(message, optional, tag = "1")]
        pub packet: Option<Packet>,
        #[prost(message, optional, tag = "2")]
        pub acknowledgement: Option<Acknowledgement>,
        #[prost(bytes = "vec", tag = "3")]
        pub proof_acked: Vec<u8>,
        #[prost(message, optional, tag = "4")]
        pub proof_height: Option<ibc_proto::ibc::core::client::v1::Height>,
        #[prost(string, tag = "5")]
        pub signer: String,
    }

    impl prost::Name for MsgAcknowledgement {
        const NAME: &'static str = "MsgAcknowledgement";
        const PACKAGE: &'static str = "ibc.core.channel.v2";
        fn full_name() -> String {
            "ibc.core.channel.v2.MsgAcknowledgement".into()
        }
        fn type_url() -> String {
            "/ibc.core.channel.v2.MsgAcknowledgement".into()
        }
    }
}

/// Client v2 types from `ibc.core.client.v2`.
pub mod client {
    use prost::Message;

    #[derive(Clone, PartialEq, Eq, Message)]
    pub struct MsgRegisterCounterparty {
        #[prost(string, tag = "1")]
        pub client_id: String,
        #[prost(bytes = "vec", repeated, tag = "2")]
        pub counterparty_merkle_prefix: Vec<Vec<u8>>,
        #[prost(string, tag = "3")]
        pub counterparty_client_id: String,
        #[prost(string, tag = "4")]
        pub signer: String,
    }

    impl prost::Name for MsgRegisterCounterparty {
        const NAME: &'static str = "MsgRegisterCounterparty";
        const PACKAGE: &'static str = "ibc.core.client.v2";
        fn full_name() -> String {
            "ibc.core.client.v2.MsgRegisterCounterparty".into()
        }
        fn type_url() -> String {
            "/ibc.core.client.v2.MsgRegisterCounterparty".into()
        }
    }
}
