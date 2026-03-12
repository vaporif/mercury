# Adding a New Chain

This guide walks through implementing a new chain backend for Mercury. The Cosmos implementation (`crates/chains/cosmos/`) is the reference — read it alongside this doc.

## Overview

A chain implementation is a crate in `crates/chains/<name>/` that defines a concrete chain struct and implements ~35 traits from `mercury-chain-traits`. Once implemented, you wire it into the CLI config and `ConnectedChain` enum.

## 1. Create the Crate

```bash
cargo init crates/chains/mychain --lib
```

Add it to the workspace in the root `Cargo.toml`:

```toml
members = [
    # ...
    "crates/chains/mychain",
]
```

Add `mercury-chain-traits` and `mercury-core` as dependencies:

```toml
[dependencies]
mercury-chain-traits = { path = "../../chain-traits" }
mercury-core = { path = "../../core" }
async-trait = { workspace = true }
eyre = { workspace = true }
tracing = { workspace = true }
tokio = { workspace = true }
serde = { workspace = true }
# + chain-specific deps (RPC clients, crypto libs, protobuf types, etc.)
```

## 2. Module Structure

Follow the same layout as Cosmos:

| Module | Purpose |
|--------|---------|
| `chain.rs` | Main struct + constructor |
| `config.rs` | `Deserialize` config struct for TOML |
| `keys.rs` | Signer trait + key pair implementation |
| `types.rs` | Domain types (events, packets, messages, proofs) |
| `status.rs` | `CanQueryChainStatus` impl |
| `queries.rs` | Client/consensus state queries |
| `packet_queries.rs` | Packet commitment/receipt/ack queries with proofs |
| `events.rs` | `CanExtractPacketEvents` — parse SendPacket/WriteAck from raw events |
| `message_builders.rs` | Build create/update client + register counterparty messages |
| `packet_builders.rs` | Build recv/ack/timeout packet messages |
| `payload_builders.rs` | Build create/update client payloads (counterparty side) |
| `messaging.rs` | `CanSendMessages` — batch + submit transactions |
| `tx.rs` | Transaction signing, fee estimation, nonce queries |

## 3. Define Domain Types

Create concrete types for all associated types required by the traits. These wrap your chain's native types:

```rust
// types.rs

#[derive(Clone, Debug)]
pub struct MyEvent { /* chain-native event fields */ }

#[derive(Clone, Debug)]
pub struct MyMessage { /* chain-native message/tx fields */ }

#[derive(Clone, Debug)]
pub struct MyTxResponse { /* tx hash, height, events */ }

#[derive(Clone, Debug)]
pub struct MyChainStatus {
    pub height: u64,
    pub timestamp: u64,
}

#[derive(Clone, Debug)]
pub struct MyPacket {
    pub source_client_id: String,
    pub dest_client_id: String,
    pub sequence: u64,
    pub timeout_timestamp: u64,
    pub payloads: Vec<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct MyCommitmentProof(pub Vec<u8>);

// PacketCommitment, PacketReceipt, Acknowledgement, ClientState, ConsensusState...
```

## 4. Define the Chain Struct

```rust
// chain.rs

#[derive(Clone, Debug)]
pub struct MyChain<S: MySigner> {
    pub config: MyChainConfig,
    pub chain_id: String,
    pub rpc_client: MyRpcClient,
    pub signer: S,
    pub block_time: Duration,
}

impl<S: MySigner> MyChain<S> {
    pub async fn new(config: MyChainConfig, signer: S) -> eyre::Result<Self> {
        // Connect to the RPC endpoint
        // Fetch chain ID from the node
        // Return initialized chain
    }
}
```

## 5. Implement Traits

### Type Traits (6)

These define your associated types. Implement all of them:

```rust
impl<S: MySigner> HasChainTypes for MyChain<S> {
    type Height = u64;          // or a newtype
    type Timestamp = u64;       // or a newtype
    type ChainId = String;
    type Event = MyEvent;
}

impl<S: MySigner> HasMessageTypes for MyChain<S> {
    type Message = MyMessage;
    type MessageResponse = MyTxResponse;
}

impl<S: MySigner> HasIbcTypes<Self> for MyChain<S> {
    type ClientId = String;     // or a domain-specific type
    type ClientState = MyClientState;
    type ConsensusState = MyConsensusState;
    type CommitmentProof = MyCommitmentProof;
}

impl<S: MySigner> HasPacketTypes<Self> for MyChain<S> {
    type Packet = MyPacket;
    type PacketCommitment = MyPacketCommitment;
    type PacketReceipt = MyPacketReceipt;
    type Acknowledgement = MyAcknowledgement;

    fn packet_sequence(packet: &MyPacket) -> u64 { packet.sequence }
    fn packet_timeout_timestamp(packet: &MyPacket) -> u64 { packet.timeout_timestamp }
}

impl<S: MySigner> HasChainStatusType for MyChain<S> {
    type ChainStatus = MyChainStatus;
    fn chain_status_height(status: &MyChainStatus) -> &u64 { &status.height }
    fn chain_status_timestamp(status: &MyChainStatus) -> &u64 { &status.timestamp }
    fn chain_status_timestamp_secs(status: &MyChainStatus) -> u64 { status.timestamp }
}

impl<S: MySigner> HasRevisionNumber for MyChain<S> {
    fn revision_number(&self) -> u64 { 0 }
}
```

### Query Traits (7)

Each is an async method. Add `#[instrument]` for tracing:

- `CanQueryChainStatus` — get current height + timestamp from RPC
- `CanQueryClientState<Self>` — query the IBC client state stored on-chain
- `CanQueryConsensusState<Self>` — query consensus state at a given height
- `HasClientLatestHeight<Self>` — extract latest height from client state
- `HasTrustingPeriod<Self>` — extract trusting period from client state
- `CanQueryBlockEvents` — poll block events and provide height incrementing logic
- `CanQueryPacketCommitment<Self>`, `CanQueryPacketReceipt<Self>`, `CanQueryPacketAcknowledgement<Self>` — query packet state with Merkle proofs

The packet query traits must return proofs alongside the data. This is chain-specific (ABCI proofs for Cosmos, storage proofs for EVM, etc.).

### Event Extraction (1)

```rust
impl<S: MySigner> CanExtractPacketEvents<Self> for MyChain<S> {
    type SendPacketEvent = MySendPacketEvent;
    type WriteAckEvent = MyWriteAckEvent;

    fn try_extract_send_packet_event(event: &MyEvent) -> Option<MySendPacketEvent> {
        // Parse chain-native event into a SendPacket event
    }

    fn try_extract_write_ack_event(event: &MyEvent) -> Option<MyWriteAckEvent> {
        // Parse chain-native event into a WriteAck event
    }

    fn packet_from_send_event(event: &MySendPacketEvent) -> &MyPacket { &event.packet }

    fn packet_from_write_ack_event(event: &MyWriteAckEvent) -> (&MyPacket, &MyAcknowledgement) {
        (&event.packet, &event.ack)
    }
}
```

### Message Builders (4)

- `CanBuildCreateClientMessage<Self>` — build a `MsgCreateClient` from a counterparty payload
- `CanBuildUpdateClientMessage<Self>` — build `MsgUpdateClient` messages from a counterparty payload
- `CanRegisterCounterparty<Self>` — build a `MsgRegisterCounterparty` message

### Packet Builders (3)

- `CanBuildReceivePacketMessage<Self>` — build a receive packet message with proof
- `CanBuildAckPacketMessage<Self>` — build an ack message with proof
- `CanBuildTimeoutPacketMessage<Self>` — build a timeout message with proof

### Payload Builders (2)

These run on the *counterparty* side to produce payloads consumed by message builders:

- `CanBuildCreateClientPayload<Self>` — build the payload for client creation
- `CanBuildUpdateClientPayload<Self>` — build the payload (usually a header + proof) for client update

### Transaction Traits (5)

- `HasTxTypes` — define `Signer`, `Nonce`, `Fee`, `TxHash` types
- `CanSubmitTx` — sign and broadcast a transaction
- `CanEstimateFee` — estimate gas/fees for a set of messages
- `CanQueryNonce` — get the current account nonce/sequence
- `CanPollTxResponse` — poll for transaction confirmation

### Messaging (1)

```rust
impl<S: MySigner> CanSendMessages for MyChain<S> {
    async fn send_messages(&self, messages: Vec<MyMessage>) -> Result<Vec<MyTxResponse>> {
        // 1. Query nonce
        // 2. Estimate fee
        // 3. Submit transaction
        // 4. Poll for confirmation
        // 5. Handle nonce mismatch retries
    }
}
```

## 6. Configuration

Define a config struct that deserializes from TOML:

```rust
// config.rs

#[derive(Clone, Debug, Deserialize)]
pub struct MyChainConfig {
    pub chain_id: String,
    pub rpc_addr: String,
    pub key_file: PathBuf,
    // chain-specific fields...

    #[serde(default = "default_block_time")]
    pub block_time: Duration,
}

impl MyChainConfig {
    pub fn validate(&self) -> eyre::Result<()> {
        // Validate required fields
    }
}
```

## 7. Wire Into the CLI

Three files need changes in `crates/cli/`:

### `config.rs`

Add your variant to the `ChainConfig` enum and match arms:

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ChainConfig {
    #[serde(rename = "cosmos")]
    Cosmos(CosmosChainConfig),
    #[serde(rename = "mychain")]
    MyChain(MyChainConfig),
}
```

### `main.rs`

Add to `ConnectedChain` enum:

```rust
enum ConnectedChain {
    Cosmos(CosmosChain<Secp256k1KeyPair>),
    MyChain(MyChain<MyKeyPair>),
}
```

Add to `connect_chain()` — initialize RPC client + signer.

Add match arms to `spawn_relay_pair()` for all chain pair combinations (MyChain↔MyChain, MyChain↔Cosmos, etc.).

## 8. Cross-Chain Relaying

To relay between your chain and an existing chain (e.g., Cosmos), both sides need `HasIbcTypes` implemented for the counterparty:

```rust
// In your chain crate:
impl<S: MySigner> HasIbcTypes<CosmosChain<CS>> for MyChain<S>
where CS: CosmosSigner
{
    type ClientId = ...;
    type ClientState = ...;    // Cosmos light client state as seen by your chain
    type ConsensusState = ...;
    type CommitmentProof = ...;
}

// In the cosmos crate (or a glue crate):
impl<S: CosmosSigner> HasIbcTypes<MyChain<MS>> for CosmosChain<S>
where MS: MySigner
{
    type ClientId = ...;
    type ClientState = ...;    // Your chain's light client state as seen by Cosmos
    type ConsensusState = ...;
    type CommitmentProof = ...;
}
```

Same pattern applies to `HasPacketTypes`, payload builders, and message builders that take a counterparty generic.

## Checklist

- [ ] Crate created and added to workspace
- [ ] Domain types defined
- [ ] Chain struct with constructor
- [ ] Signer trait + at least one key pair implementation
- [ ] All 6 type traits implemented
- [ ] All query traits implemented (chain status, client state, consensus state, block events, packet queries)
- [ ] Event extraction (SendPacket, WriteAck parsing)
- [ ] Message builders (create/update client, register counterparty)
- [ ] Packet builders (recv, ack, timeout)
- [ ] Payload builders (create/update client payloads)
- [ ] Transaction traits (submit, estimate fee, query nonce, poll response)
- [ ] `CanSendMessages` with batching and nonce retry logic
- [ ] Config struct with TOML deserialization and validation
- [ ] CLI wired up (`ChainConfig` enum, `ConnectedChain`, `connect_chain`, `spawn_relay_pair`)
- [ ] Cross-chain trait impls if relaying to other chain types
- [ ] Tests (unit + integration)
