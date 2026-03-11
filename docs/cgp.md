# Why Not CGP

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

