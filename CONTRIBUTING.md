# Contributing

## Prerequisites

### With Nix (recommended)

The project includes a Nix flake with a complete dev shell. If you have Nix with flakes enabled:

```bash
# Enter the dev shell (or use direnv with the included .envrc)
nix develop

# Build the binary via Nix
nix build
```

The dev shell provides the stable Rust toolchain (cargo, clippy, rustfmt, rust-analyzer), plus `cargo-nextest`, `taplo`, `typos`, `just`, and `actionlint`.

### Without Nix

Requires a stable Rust toolchain. Install via [rustup](https://rustup.rs).

You'll also need these tools (provided automatically by the Nix dev shell):

- [just](https://github.com/casey/just) - command runner
- [cargo-nextest](https://nexte.st) - test runner for E2E tests
- [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) - dependency auditing
- [taplo](https://taplo.tamasfe.dev) - TOML formatter/linter
- [typos](https://github.com/crate-ci/typos) - spell checker
- [actionlint](https://github.com/rhysd/actionlint) - GitHub Actions linter
- A container runtime - required for E2E tests. Any OCI-compatible runtime works: [Docker](https://docs.docker.com/get-docker/), [Podman](https://podman.io), [OrbStack](https://orbstack.dev) (macOS), [colima](https://github.com/abiosoft/colima), [nerdctl](https://github.com/containerd/nerdctl)

## Cloning

This repo uses git submodules. Clone with:

```bash
git clone --recurse-submodules https://github.com/vaporif/mercury.git
```

To auto-pull submodules on future `git pull`/`git checkout`:

```bash
git config submodule.recurse true
```

## Building and testing

A `justfile` wraps all common commands:

```bash
just check        # Run all checks (typos, TOML lint, fmt, clippy, test)
just test         # Run unit tests
just clippy       # Run clippy
just fmt          # Format check
just fmt-fix      # Auto-format everything (Rust, TOML, Nix)
just e2e          # Run E2E tests (requires container runtime)
just check-typos  # Spell check
just check-toml   # TOML lint
```

## Code style

### Rust

- Edition 2024, MSRV `1.91.0`
- Formatted with `cargo fmt` - non-negotiable
- Clippy with `all`, `pedantic`, `nursery`, and `cargo` lint groups enabled - zero warnings (`-D warnings`). Suppress individual lints with `#[allow]` and a justification comment
- Modern module syntax: `foo.rs` + `foo/bar.rs` - never `foo/mod.rs`
- Prefer `impl Trait` in args/returns over `Box<dyn Trait>` where possible
- Use iterators and combinators (`.map`, `.filter`, `.collect`) over manual loops
- `eyre` for error handling with retryability tracking - propagate with `?`, no `.unwrap()` or `.expect()` in production code
- `tracing` for structured logging - not `println!` or `log`
- `async-trait` for async trait methods

### Formatting

- TOML - `taplo`
- Nix - `alejandra`

### AI-assisted contributions

AI assistants are welcome as tools. The human contributor bears full responsibility for every line submitted - correctness, licensing, and review. If you used AI to generate code, you must have read and verified it yourself before opening a PR. **Unreviewed AI output will be declined.**

## Understanding the codebase

Read the [Architecture](./docs/architecture.md) doc before diving into the code. It covers the trait hierarchy, worker pipeline, crate boundaries, and design decisions.

### Crate map

| Crate | Description |
|-------|-------------|
| `mercury-cli` (`crates/cli`) | CLI binary - entry point, config parsing, worker orchestration |
| `mercury-core` (`crates/core`) | Error types, encoding, plugin traits, worker trait, membership proofs |
| `mercury-chain-traits` (`crates/chain-traits`) | Chain types, messaging, queries, relay traits (traits) |
| `mercury-relay` (`crates/relay`) | Worker pipeline, generic over chain traits |
| `mercury-chain-cache` (`crates/chain-cache`) | Query result caching + tx coordination (deduplication) |
| `mercury-telemetry` (`crates/telemetry`) | Metrics, logging, worker gauges |
| `mercury-cosmos` (`crates/chains/core/cosmos`) | Cosmos chain implementation - RPC, protobuf, tx signing |
| `mercury-ethereum` (`crates/chains/core/ethereum`) | EVM chain implementation - alloy, contracts, SP1 proving |
| `mercury-cosmos-counterparties` (`crates/chains/counterparties/cosmos`) | Cosmos adapter + cross-chain trait impls |
| `mercury-ethereum-counterparties` (`crates/chains/counterparties/ethereum`) | Ethereum adapter + cross-chain trait impls |
| `mercury-cosmos-cosmos-relay` (`crates/chains/relay-pairs/cosmos-cosmos`) | Cosmos↔Cosmos relay pair plugin |
| `mercury-cosmos-ethereum-relay` (`crates/chains/relay-pairs/cosmos-ethereum`) | Cosmos↔Ethereum relay pair plugin |
| `mercury-e2e` (`crates/e2e`) | End-to-end tests |

### Entry points

- Adding a chain? Start with [Adding a new chain](./docs/adding-a-chain.md) and use `crates/chains/cosmos/` as reference.
- Understanding the relay pipeline? Read `crates/relay/src/workers/` - each worker is a self-contained module.
- Working on traits? All chain abstractions live in `crates/chain-traits/src/`.

### Design principles

- Plain traits, no frameworks. Direct `impl` blocks, no provider indirection or macro-heavy abstractions.
- Few, focused traits. Traits grouped by concern - `ChainTypes` and `IbcTypes` carry associated types, not one trait per type.
- Concrete error type. One `eyre`-based error with retryability tracking, no generic error parameters.
- Plugin-based chain extensibility. Chains register via `ChainPlugin` + `RelayPairPlugin` traits into a `ChainRegistry`. Adding a new chain requires no CLI changes - implement the plugin traits and call `register()`.
- Don't abstract implementation details. Transaction internals (fees, nonces, signing) are concrete methods on chain types, not traits.
