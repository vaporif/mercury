# Why Rewrite

There are two existing IBC relayers in Rust: the [original Hermes](https://github.com/informalsystems/hermes) and its intended successor [Hermes SDK](https://github.com/informalsystems/hermes-sdk). Hermes has been reliable production software for years, but its architecture makes certain improvements difficult. Hermes SDK attempted to address this through CGP, which introduced its own problems. Mercury takes a different path.

## Hermes: Architectural Constraints

Hermes was designed around a synchronous, thread-based model — a [reasonable choice in 2020](https://github.com/informalsystems/hermes/issues/121). Rust had no native `async fn` in traits (stabilized in late 2023), tokio 1.0 hadn't shipped yet, and the async runtime ecosystem was fragmented. The team explicitly chose to start synchronous — get the relaying logic correct and testable before introducing concurrency. Synchronous code was easier to reason about for multi-step relay workflows involving cancellation and complex state transitions.

In practice, this means the `ChainHandle` trait defines ~65 synchronous methods, each dispatched through crossbeam channels to a `ChainRuntime` running a `crossbeam_channel::select!` loop in a dedicated OS thread. Async operations (gRPC, RPC) are executed through a `block_on` bridge — tokio exists only as an internal detail while all orchestration remains synchronous. Each IBC channel gets its own worker thread, so operators relaying many channels end up spawning hundreds of OS threads instead of lightweight async tasks.

The chain abstractions are also tightly coupled to Cosmos SDK semantics, making non-Cosmos chain support harder than it needs to be.

By the time async Rust matured, the sync model was load-bearing and too costly to retrofit. The existence of hermes-sdk as a separate ground-up effort suggests the team reached the same conclusion. Mercury benefits from starting in 2026, where stable async traits, a mature tokio, and battle-tested async patterns are the default.

## Hermes SDK: Right Problem, Wrong Abstraction

[Hermes SDK](https://github.com/informalsystems/hermes-sdk) was built on [Context-Generic Programming](https://github.com/contextgeneric/cgp) (CGP), a custom Rust framework that provides compile-time polymorphism without runtime dispatch.

The problems CGP targets are real and important. Hermes v1's monolithic `ChainHandle` trait was hardcoded to Cosmos, and IBC is expanding to Ethereum, Solana, Starknet, Sovereign rollups, Substrate — chains with fundamentally different APIs, signing, and proof systems. More critically, cross-chain relaying hits a structural limitation in Rust's trait system: when `ClientMessageBuilder<Counterparty>` requires `Counterparty: ClientPayloadBuilder<Self>`, implementing a Cosmos→EVM bridge forces the Cosmos crate to depend on the EVM crate and vice versa — a circular dependency that Cargo prohibits, compounded by Rust's orphan rule which prevents third-party crates from providing the impls.

CGP solves this through Inversion of Control — decoupling producer types from consumer types so that each chain can be implemented independently. This is the correct insight. The issue is that CGP wraps it in a custom macro framework that makes the codebase difficult to contribute to.

## What CGP Actually Is

CGP is an Inversion of Control container — the kind you'd find in Java's Spring or .NET's Autofac — but implemented entirely through Rust proc macros. `#[cgp_component]` generates trait impls, `#[cgp_provider]` generates delegation glue, `#[cgp_context]` generates the container struct, and `delegate_components!` is the registration DSL.

For every operation, CGP introduces three layers of indirection:

1. **Component trait** — defines the interface (e.g., `CanSendMessages`)
2. **Provider** — a zero-sized type that implements the trait for a specific context
3. **Delegation table** — a macro-generated mapping from component names to providers

A concrete chain type like `CosmosChain` doesn't implement `CanSendMessages` directly. Instead, `#[cgp_context]` generates a components struct, `delegate_components!` maps `MessageSenderComponent` to a provider type, and that provider implements `MessageSender<CosmosChain>`. The actual trait impl is synthesized by proc macros.

```rust
// CGP: 367 component traits, 666 provider impls, 25 context types,
// all wired through macro-generated delegation tables

#[cgp_component {
  provider: MessageSender,
  context: Chain,
}]
#[async_trait]
pub trait CanSendMessages: HasMessageType + HasMessageResponseType + HasAsyncErrorType {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<Vec<Self::MessageResponse>, Self::Error>;
}

delegate_components! {
    CosmosChainContextComponents {
        MessageSenderComponent: SomeProviderType,
        // ... 30+ more component mappings
    }
}
```

Macros take other macros' output as input, expanding into thousands of lines of generated code that's difficult to read or debug.

## Where It Breaks Down

**Compile times.** Every component definition runs proc macros that generate trait impls, blanket impls, and delegation boilerplate. With 367 components and 666 providers, compilation is dominated by macro expansion. CGP also requires increasing `#![recursion_limit]` beyond the default, which suggests the trait resolution is pushing beyond what the macro architecture handles cleanly.

**Error messages.** When a trait bound isn't satisfied, the compiler reports errors through layers of macro-generated types. Instead of "CosmosChain doesn't implement CanSendMessages", you get pages of errors about `DelegateComponent`, `UseDelegate`, `WithProvider`, and generated component structs that don't exist in your source code.

**Cognitive overhead.** To understand how messages are sent on Cosmos, you need to: find the component trait, find the provider mapped in the delegation table, find the provider's impl block, and trace through any nested delegation. Four files minimum for a single operation. This indirection is unnecessary when a direct `impl` block achieves the same result.

**Tooling.** rust-analyzer cannot resolve through CGP's macro layers. Go-to-definition lands on the macro attribute, not the implementation. Autocomplete and rename refactoring don't work across the generated boundaries.

**Onboarding.** CGP has a small community, so new contributors must learn a custom programming paradigm before they can read the code. The learning curve goes beyond Rust traits — it includes the macro framework and its dispatch mechanism on top.

## What Rust Already Has

Rust's traits and generics already provide compile-time polymorphism. Mercury uses these native mechanisms directly:

```rust
// Mercury: one trait, one impl, zero macros

#[async_trait]
pub trait MessageSender: ChainTypes {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<Vec<Self::MessageResponse>>;
}

#[async_trait]
impl MessageSender for CosmosChainInner {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<Vec<Self::MessageResponse>> {
        let tx_bytes = self.encode_and_sign(messages).await?;
        let response = self.rpc_client.broadcast_tx_sync(tx_bytes).await?;
        self.poll_for_tx(response.hash).await
    }
}
```

rust-analyzer works. Error messages point to your code. Go-to-definition goes to the implementation. Any Rust developer can read it.

## Why Few Traits Instead of Many

CGP takes decomposition to the extreme — every associated type gets its own trait (`HasHeightType`, `HasTimestampType`, `HasMessageType`, `HasChainIdType`, ...). This maximizes theoretical composability but in practice you never implement `HasHeightType` without also implementing `HasTimestampType`. They always appear together. The result is where clauses listing 10+ trait bounds that always co-occur, and hundreds of single-type traits that add indirection without adding flexibility.

Mercury consolidates co-occurring types into two traits: `ChainTypes` (all chain-local types: height, timestamp, client ID, messages, chain status) and `IbcTypes` (all IBC-specific types: client state, packets, proofs, acknowledgements). Within each group, the types are always needed together, so separating them adds complexity without enabling any real composition.

## Eureka Relayer: Different Layer

The [Eureka relayer](https://github.com/cosmos/solidity-ibc-eureka/tree/main/programs/relayer) is a stateless gRPC service that generates unsigned transactions for cross-chain IBC operations. It doesn't monitor chains, submit transactions, or manage state — a separate orchestrator handles that. Each chain pair is an independent module with no shared relay logic.

Mercury is a full relayer with shared relay logic across all chain pairs. The architectures are complementary — Mercury could use the Eureka relayer's gRPC API as a transaction generation backend.

## Cross-Chain Without the Abstraction Tax

CGP's core insight — Inversion of Control to decouple cross-chain trait bounds — is correct. Mercury applies the same principle directly in Rust's trait system, without a macro framework. The solution combines three techniques:

### Non-Generic IBC Types

The original approach parameterized `IbcTypes` over a counterparty chain: `IbcTypes<Counterparty: ChainTypes>`. This meant implementing Cosmos→EVM relay forced `CosmosChain: IbcTypes<EthereumChain>`, which had to live in the Cosmos crate (orphan rule), creating a circular dependency.

Mercury removes the generic entirely. `IbcTypes` is a plain supertrait of `ChainTypes`:

```rust
// Before: counterparty-parameterized
pub trait IbcTypes<Counterparty: ChainTypes + ?Sized>: ChainTypes {
    type ClientState: Clone + Debug + ThreadSafe;
    // ...
}

// After: non-generic, counterparty-independent
pub trait IbcTypes: ChainTypes {
    type ClientState: Clone + Debug + ThreadSafe;
    // ...
}
```

This works because in practice a chain's IBC types don't change based on the counterparty. Cosmos uses the same `ClientState`, `Packet`, and `CommitmentProof` types regardless of whether the counterparty is another Cosmos chain or an EVM chain. The generic parameter added type-level precision that never manifested as real variation.

### Wrapper Pattern for Orphan Rule

Cross-chain trait impls need to be local to *some* crate. Mercury uses counterparty crates with wrapper types:

```
mercury-cosmos         → CosmosChainInner<S>  (core impl)
mercury-cosmos-counterparties → CosmosChain<S>       (wrapper, cross-chain impls)
mercury-ethereum       → EthereumChainInner   (core impl)
mercury-ethereum-counterparties → EthereumChain      (wrapper, cross-chain impls)
```

The wrapper type is local to its counterparty crate, so it can implement traits for any counterparty type without violating the orphan rule. `HasInner` constrains all associated types to match, so relay code seamlessly passes values between wrapper and inner contexts.

### Weakened Builder Bounds

Builder traits require only `Counterparty: ChainTypes`, not `Counterparty: IbcTypes`. Types that cross the chain boundary become associated types on the consuming trait:

```rust
pub trait ClientMessageBuilder<Counterparty: ChainTypes>: IbcTypes {
    type CreateClientPayload: ThreadSafe;
    type UpdateClientPayload: ThreadSafe;
    // ...
}
```

The relay composition site enforces that producer and consumer types match:

```rust
pub trait Relay: ThreadSafe {
    type SrcChain: RelayChain
        + ClientPayloadBuilder<
            <Self::DstChain as HasInner>::Inner,
            UpdateClientPayload = <Self::DstChain as ClientMessageBuilder<
                <Self::SrcChain as HasInner>::Inner,
            >>::UpdateClientPayload,
        >;
    // ...
}
```

Each chain declares its own types; the compiler verifies they match when wired together. No macros, no provider indirection, no delegation tables. The same IoC pattern, expressed through Rust's trait system directly.

The source chain produces payloads (`ClientPayloadBuilder`), the destination chain consumes them and bridges the type systems (`ClientMessageBuilder`, `PacketMessageBuilder`, `ClientQuery`). The source never needs to know the destination's IBC types. Cross-chain impls live in the destination chain's counterparty crate behind a feature flag, and the source chain crate remains independent.
