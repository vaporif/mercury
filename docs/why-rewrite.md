# Why rewrite

Two IBC relayers exist in Rust already: [Hermes](https://github.com/informalsystems/hermes) and [Hermes SDK](https://github.com/informalsystems/hermes-sdk). Hermes has been solid production software for years, but its architecture makes certain things hard to change. Hermes SDK tried to fix this with CGP, which brought its own problems. Mercury goes a different direction.

## Hermes: architectural constraints

Hermes was designed around a synchronous, thread-based model - a [reasonable choice in 2020](https://github.com/informalsystems/hermes/issues/121). Rust had no native `async fn` in traits (stabilized in late 2023), tokio 1.0 hadn't shipped yet, and the async runtime ecosystem was fragmented. The team explicitly chose to start synchronous - get the relaying logic correct and testable before introducing concurrency. Synchronous code was easier to reason about for multi-step relay workflows involving cancellation and complex state transitions.

In practice, the `ChainHandle` trait defines ~65 synchronous methods, each dispatched through crossbeam channels to a `ChainRuntime` running a `crossbeam_channel::select!` loop in a dedicated OS thread. Async operations (gRPC, RPC) go through a `block_on` bridge - tokio exists only as an internal detail while all orchestration stays synchronous. Each IBC channel gets its own worker thread, so operators relaying many channels end up spawning hundreds of OS threads instead of lightweight async tasks.

The chain abstractions are also tightly coupled to Cosmos SDK semantics, making non-Cosmos chain support harder than it needs to be.

By the time async Rust matured, the sync model was load-bearing and too costly to retrofit. That hermes-sdk exists as a separate ground-up effort says it all. Mercury gets to start in 2026 where stable async traits and a mature tokio are just the default.

### The fork problem

Hermes hardcodes chain types into core enums (`ChainConfig::CosmosSdk`, `ChainConfig::Namada`, `ChainConfig::Penumbra`) and dispatches through match arms spread across the relayer crate (~280 chain-specific references). Adding a new chain means modifying Hermes internals. In practice, chain teams maintain forks - Namada runs a [fork](https://github.com/heliaxdev/hermes) with Namada-specific changes that must be continuously rebased on upstream.

Mercury's plugin architecture eliminates this. The relay pipeline, CLI, and core crate contain zero chain-specific code. Adding a chain is additive: create new crates under `crates/chains/`, implement the plugin traits, add register calls in `bin/src/registry.rs`. No enums to extend, no match arms to update, no fork to maintain. Upstream updates never conflict with chain-specific code because they never touch the same files.

## Hermes SDK: right problem, wrong abstraction

[Hermes SDK](https://github.com/informalsystems/hermes-sdk) was built on [Context-Generic Programming](https://github.com/contextgeneric/cgp) (CGP), a custom Rust framework for compile-time polymorphism without runtime dispatch.

The problems CGP targets are real. Hermes v1's monolithic `ChainHandle` trait was hardcoded to Cosmos, and IBC is expanding to Ethereum, Solana, Starknet, Sovereign rollups, Substrate - chains with fundamentally different APIs, signing, and proof systems. More critically, cross-chain relaying hits a structural limitation in Rust's trait system: when `ClientMessageBuilder<Counterparty>` requires `Counterparty: ClientPayloadBuilder<Self>`, implementing a Cosmos→EVM bridge forces the Cosmos crate to depend on the EVM crate and vice versa - a circular dependency that Cargo prohibits, compounded by Rust's orphan rule which prevents third-party crates from providing the impls.

CGP solves this through Inversion of Control - decoupling producer types from consumer types so that each chain can be implemented independently. This is the correct insight. The issue is that CGP wraps it in a custom macro framework that makes the codebase hard to contribute to. When a framework requires bumping `#![recursion_limit]` past the compiler default, it's fighting the language rather than working with it.

## Where CGP breaks down

CGP is essentially an Inversion of Control container (like Java's Spring) implemented through Rust proc macros. For every operation it introduces three layers: a component trait, a provider type, and a macro-generated delegation table. The result is 367 component traits, 666 provider impls, and 25 context types - all wired through macro-generated code.

In practice:

- Compile times dominated by macro expansion across hundreds of components
- Error messages report through layers of generated types (`DelegateComponent`, `UseDelegate`, `WithProvider`) instead of pointing to your code
- Understanding a single operation requires tracing through four files: component trait → delegation table → provider → impl
- rust-analyzer can't resolve through the macro layers; go-to-definition, autocomplete, and rename don't work
- Contributors must learn a custom programming paradigm before they can read the code

## Mercury's approach

The core insight behind CGP - Inversion of Control to decouple cross-chain trait bounds - is right. Mercury applies it directly in Rust's trait system without the macro framework: non-generic `IbcTypes`, adapter types for orphan rule avoidance (generated via `delegate_chain!`), and weakened builder bounds. rust-analyzer works. Error messages point to your code. Any Rust developer can read it.

See [architecture](./architecture.md) for the full trait hierarchy, cross-chain design, and code examples.

## Cosmos Labs relayer: request-driven and no longer open

Cosmos Labs splits IBC v2 relaying into two services. The [proof API](https://github.com/cosmos/ibc-contracts/tree/main/programs/proof-api) (renamed from `programs/relayer` in [#1030](https://github.com/cosmos/ibc-contracts/pull/1030)) is a stateless gRPC service. Given source tx hashes, it returns unsigned relay transactions with proofs. It doesn't watch chains or submit anything.

The [IBC v2 Relayer](https://github.com/cosmos/ibc-relayer) is a Go service on top of it, backed by Postgres. A client calls its `Relay(tx_hash, chain_id)` endpoint. The relayer stores the packets from that transaction, gets the relay transactions from the proof API, signs them, and submits them. It covers many production concerns: retries after reorgs and gas failures, batching, remote signing, address blacklists (for OFAC), and gas and latency metrics. Its README lists Cosmos and EVM chains.

The public repo was archived in August 2026. Cosmos Labs now licenses the relayer through [Cosmos Enterprise](https://cosmos.network/blog/flexible-and-secure-interoperability-for-financial-institutions), so Cosmos Labs no longer maintains an open-source IBC v2 relayer.

Where Mercury differs:

- Autonomous operation. The Cosmos Labs relayer only relays transactions that a client reports to its API, so a packet nobody reports stays unrelayed. Mercury watches blocks, sweeps for missed packets, refreshes clients, and submits transactions on its own.
- Fewer moving parts. Their stack needs Postgres, a proof API instance, and a client that feeds it tx hashes. Mercury builds and submits transactions itself, with no database or proof service to run.
- Shared relay logic. The proof API has a separate module per direction (`eth-to-cosmos`, `cosmos-to-eth`, `solana-to-cosmos`, and so on), and each module parses its config from `serde_json::Value` at runtime. Mercury's workers (`EventWatcher`, `PacketWorker`, `TxWorker`, `PacketSweeper`) are generic over the `Relay` trait, so one pipeline serves every chain pair. `RelayContext<Src, Dst>` checks payload types at compile time. Mercury still needs a counterparty wrapper per chain pair (~600 lines each) because of the orphan rule.
- Open source under MIT/Apache-2.0.

Mercury lacks what the Cosmos Labs relayer has from running in production: remote signing, address filtering, tested retry handling, and live deployments.
