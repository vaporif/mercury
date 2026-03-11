# Architecture

Mercury is an IBC relayer built with plain Rust traits and generics. No macro frameworks, no code generation, no custom programming paradigms.

## Design Principles

- **Direct trait impls.** Every chain operation is a trait method with a direct `impl` block on the concrete type. No provider indirection.
- **Small, focused traits.** ~35 traits grouped by concern instead of 250+ component traits.
- **Concrete error type.** One error type based on `eyre::Report` with retryability tracking. No generic error parameters on traits.
- **Struct fields, not trait getters.** Configuration, runtime handles, and RPC clients are struct fields accessed via methods. Not abstracted behind traits.

## Trait Hierarchy

### Type Traits

```rust
pub trait HasChainTypes: Send + Sync + 'static {
    type Height: Clone + Ord + Debug + Display + Send + Sync + 'static;
    type Timestamp: Clone + Ord + Debug + Send + Sync + 'static;
    type ChainId: Clone + Debug + Display + Send + Sync + 'static;
    type Event: Clone + Debug + Send + Sync + 'static;
}

pub trait HasMessageTypes: HasChainTypes {
    type Message: Send + Sync + 'static;
    type MessageResponse: Send + Sync + 'static;
}
```

### Counterparty Generics

IBC relaying involves two chains that know about each other's types. Chain A stores a client state *of* chain B. This cross-chain type relationship is modeled with a generic parameter:

```rust
pub trait HasIbcTypes<Counterparty: HasChainTypes>: HasChainTypes {
    type ClientId: Clone + Debug + Display + Send + Sync + 'static;
    type ClientState: Clone + Debug + Send + Sync + 'static;
    type ConsensusState: Clone + Debug + Send + Sync + 'static;
    type CommitmentProof: Clone + Send + Sync + 'static;
}
```

`CosmosChain` implements `HasIbcTypes<CosmosChain>` for Cosmos-to-Cosmos relaying, and could implement `HasIbcTypes<CelestiaChain>` with different types for Cosmos-to-Celestia. The compiler prevents mixing up source and destination types.

### Trait Groups (~35 total)

- **Type traits** (4) — `HasChainTypes`, `HasMessageTypes`, `HasIbcTypes<C>`, `HasPacketTypes<C>`
- **Query traits** (6) — chain status, client state, consensus state, packet commitments
- **Message builders** (7) — create/update client, register counterparty, recv/ack/timeout packets
- **Payload builders** (2) — create/update client payloads (counterparty side)
- **Transaction traits** (4) — submit, estimate fee, query nonce, poll response
- **Relay traits** (6) — packet relay, client update, event relay, bidirectional relay
- **Infrastructure** (2) — runtime, encoding

## Crate Layout

```
mercury-chain-traits     Pure trait definitions, no_std compatible
        |
mercury-tx-traits        Transaction submission traits
        |
mercury-relay-traits     Relay orchestration traits
        |
mercury-cosmos           Cosmos SDK implementation (RPC, protobuf, tx signing)
        |
mercury-relay            Concrete relay logic, generic over chain traits
        |
mercury-runtime          Tokio-based runtime
        |
mercury-cli              CLI binary wiring everything together
```

## Data Flow: Relaying a Packet

1. **Event loop** watches source chain for `SendPacket` events
2. **Extract** packet data from the event
3. **Update client** on destination with source chain's latest state
4. **Query proof** of packet commitment on source chain
5. **Build** `MsgRecvPacket` on destination chain (packet + proof)
6. **Submit** the message to destination chain
7. **Watch** for acknowledgement on destination
8. **Relay ack** back to source chain (ack + proof)

## Error Handling

One concrete error type (`mercury-error`) based on `eyre::Report` with retryability tracking:

```rust
pub struct MercuryError {
    inner: eyre::Report,
    retryable: bool,
}

pub type Result<T> = std::result::Result<T, MercuryError>;
```

`Result<T>` uses the project's error type everywhere. No generic error type parameters on traits.

## What's Not Abstracted

Mercury keeps these as direct code rather than trait abstractions:

- **Logging** — uses `tracing` directly
- **Field access** — struct fields accessed via methods (e.g. `self.runtime`)
- **Configuration** — config values are struct fields
- **Test infrastructure** — test setup is separate from the core trait hierarchy
