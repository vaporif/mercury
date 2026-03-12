# Why Rewrite

There are two existing IBC relayers in Rust: the [original Hermes](https://github.com/informalsystems/hermes) and its intended successor [Hermes SDK](https://github.com/informalsystems/hermes-sdk). Hermes has been reliable production software for years, but its architecture makes certain improvements difficult. Hermes SDK attempted to address this through CGP, which introduced its own problems. Mercury takes a different path.

## Hermes: Architectural Constraints

The sync-first design was a reasonable choice in 2020. Rust had no native `async fn` in traits (stabilized in late 2023), tokio 1.0 hadn't shipped yet, and the async runtime ecosystem was fragmented. The team [explicitly chose](https://github.com/informalsystems/hermes/issues/121) to start synchronous — get the relaying logic correct and testable before introducing concurrency. Synchronous code was easier to reason about for multi-step relay workflows involving cancellation and complex state transitions.

The problem is that by the time async Rust matured, the sync model was load-bearing and too costly to retrofit. Mercury benefits from starting in 2026, where stable async traits, a mature tokio, and battle-tested async patterns are the default.

Hermes was designed around a synchronous, thread-based model. The `ChainHandle` trait defines ~65 synchronous methods, each dispatched through crossbeam channels to a `ChainRuntime` running a `crossbeam_channel::select!` loop in a dedicated OS thread. Async operations (gRPC, RPC) are executed through a `block_on` bridge — tokio exists only as an internal detail while all orchestration remains synchronous.

This means each IBC channel gets its own worker thread communicating via crossbeam. Operators relaying many channels end up spawning hundreds of OS threads instead of lightweight async tasks — a [known scalability concern](https://github.com/informalsystems/hermes/issues/121) that's hard to fix without rearchitecting the core.

The chain abstractions are also tightly coupled to Cosmos SDK semantics, making non-Cosmos chain support harder than it needs to be.

These aren't things that can be fixed incrementally — the sync-first threading model is load-bearing. The existence of hermes-sdk as a separate ground-up effort suggests the team reached the same conclusion.

## Hermes SDK: CGP Complexity

[Hermes SDK](https://github.com/informalsystems/hermes-sdk) was built on [Context-Generic Programming](https://github.com/contextgeneric/cgp) (CGP), a custom Rust framework that provides compile-time polymorphism without runtime dispatch. In practice, it made the codebase difficult to contribute to.

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
pub trait CanSendMessages: HasMessageTypes {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<Vec<Self::MessageResponse>>;
}

#[async_trait]
impl CanSendMessages for CosmosChain {
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

