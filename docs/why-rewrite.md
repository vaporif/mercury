# Why rewrite

There are two existing IBC relayers in Rust: the [original Hermes](https://github.com/informalsystems/hermes) and its intended successor [Hermes SDK](https://github.com/informalsystems/hermes-sdk). Hermes has been reliable production software for years, but its architecture makes certain improvements difficult. Hermes SDK tried to fix this through CGP, which introduced its own problems. Mercury takes a different path.

## Hermes: architectural constraints

Hermes was designed around a synchronous, thread-based model — a [reasonable choice in 2020](https://github.com/informalsystems/hermes/issues/121). Rust had no native `async fn` in traits (stabilized in late 2023), tokio 1.0 hadn't shipped yet, and the async runtime ecosystem was fragmented. The team explicitly chose to start synchronous — get the relaying logic correct and testable before introducing concurrency. Synchronous code was easier to reason about for multi-step relay workflows involving cancellation and complex state transitions.

In practice, the `ChainHandle` trait defines ~65 synchronous methods, each dispatched through crossbeam channels to a `ChainRuntime` running a `crossbeam_channel::select!` loop in a dedicated OS thread. Async operations (gRPC, RPC) go through a `block_on` bridge — tokio exists only as an internal detail while all orchestration stays synchronous. Each IBC channel gets its own worker thread, so operators relaying many channels end up spawning hundreds of OS threads instead of lightweight async tasks.

The chain abstractions are also tightly coupled to Cosmos SDK semantics, making non-Cosmos chain support harder than it needs to be.

By the time async Rust matured, the sync model was load-bearing and too costly to retrofit. The existence of hermes-sdk as a separate ground-up effort suggests the team reached the same conclusion. Mercury benefits from starting in 2026, where stable async traits, a mature tokio, and battle-tested async patterns are just the default.

### The fork problem

Hermes hardcodes chain types into core enums (`ChainConfig::CosmosSdk`, `ChainConfig::Namada`, `ChainConfig::Penumbra`) and dispatches through match arms spread across the relayer crate (~280 chain-specific references). Adding a new chain means modifying Hermes internals. In practice, chain teams maintain forks — Namada runs a [fork](https://github.com/heliaxdev/hermes) with Namada-specific changes that must be continuously rebased on upstream.

Mercury's plugin architecture eliminates this. The relay pipeline, CLI, and core crate contain zero chain-specific code. Adding a chain is additive: create new crates under `crates/chains/`, implement the plugin traits, add register calls in `crates/cli/src/registry.rs`. No enums to extend, no match arms to update, no fork to maintain. Upstream updates never conflict with chain-specific code because they never touch the same files.

## Hermes SDK: right problem, wrong abstraction

[Hermes SDK](https://github.com/informalsystems/hermes-sdk) was built on [Context-Generic Programming](https://github.com/contextgeneric/cgp) (CGP), a custom Rust framework for compile-time polymorphism without runtime dispatch.

The problems CGP targets are real. Hermes v1's monolithic `ChainHandle` trait was hardcoded to Cosmos, and IBC is expanding to Ethereum, Solana, Starknet, Sovereign rollups, Substrate — chains with fundamentally different APIs, signing, and proof systems. More critically, cross-chain relaying hits a structural limitation in Rust's trait system: when `ClientMessageBuilder<Counterparty>` requires `Counterparty: ClientPayloadBuilder<Self>`, implementing a Cosmos→EVM bridge forces the Cosmos crate to depend on the EVM crate and vice versa — a circular dependency that Cargo prohibits, compounded by Rust's orphan rule which prevents third-party crates from providing the impls.

CGP solves this through Inversion of Control — decoupling producer types from consumer types so that each chain can be implemented independently. This is the correct insight. The issue is that CGP wraps it in a custom macro framework that makes the codebase hard to contribute to. When a framework requires bumping `#![recursion_limit]` past the compiler default, it's fighting the language rather than working with it.

## Where CGP breaks down

CGP is essentially an Inversion of Control container (like Java's Spring) implemented through Rust proc macros. For every operation it introduces three layers: a component trait, a provider type, and a macro-generated delegation table. The result is 367 component traits, 666 provider impls, and 25 context types — all wired through macro-generated code.

The practical costs:

- Compile times dominated by macro expansion across hundreds of components
- Error messages report through layers of generated types (`DelegateComponent`, `UseDelegate`, `WithProvider`) instead of pointing to your code
- Understanding a single operation requires tracing through four files: component trait → delegation table → provider → impl
- rust-analyzer can't resolve through the macro layers; go-to-definition, autocomplete, and rename don't work
- Contributors must learn a custom programming paradigm before they can read the code

## Mercury's approach

CGP's core insight — Inversion of Control to decouple cross-chain trait bounds — is correct. Mercury applies the same principle directly in Rust's trait system, without a macro framework: non-generic `IbcTypes`, adapter types for orphan rule avoidance (generated via the `delegate_chain!` macro), and weakened builder bounds. Direct trait impls mean rust-analyzer works, error messages point to your code, and any Rust developer can read it.

See [architecture](./architecture.md) for the full trait hierarchy, cross-chain design, and code examples.

## Eureka relayer: different layer

The [Eureka relayer](https://github.com/cosmos/solidity-ibc-eureka/tree/main/programs/relayer) is a stateless gRPC service that generates unsigned transactions for cross-chain IBC operations. Its `RelayerService` exposes `RelayByTx(source_tx_ids)` — an external system must discover IBC transactions and call this RPC with specific tx hashes. No event watching, no packet recovery, no continuous relaying. It doesn't even submit transactions — it returns raw tx bytes to the caller.

Each chain direction is a separate crate (`eth-to-cosmos`, `cosmos-to-eth`, `solana-to-cosmos`, etc. — 7 modules today). Bidirectional relay requires two independent module instances sharing no state. Module config is `serde_json::Value` parsed at runtime.

Mercury differs in three ways:

- Shared relay logic. All workers (`EventWatcher`, `PacketWorker`, `TxWorker`, `PacketSweeper`) are generic over the `Relay` trait — the same code handles every chain pair. Eureka duplicates relay logic per direction. Mercury still has per-pair counterparty wrappers (~600 lines each for orphan rule compliance), but the relay pipeline itself is written once.
- Autonomous operation. Mercury polls blocks, recovers missed packets, manages client refresh, and submits transactions. Eureka requires an external orchestrator for all of this.
- Compile-time type safety. `RelayContext<Src, Dst>` enforces payload type matching through associated type constraints. Eureka's modules receive opaque `serde_json::Value` config and discover type errors at runtime.
