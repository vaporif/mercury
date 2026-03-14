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

- [just](https://github.com/casey/just) — command runner
- [cargo-nextest](https://nexte.st) — test runner for E2E tests
- [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) — dependency auditing
- [taplo](https://taplo.tamasfe.dev) — TOML formatter/linter
- [typos](https://github.com/crate-ci/typos) — spell checker
- [actionlint](https://github.com/rhysd/actionlint) — GitHub Actions linter
- A container runtime — required for E2E tests. Any OCI-compatible runtime works: [Docker](https://docs.docker.com/get-docker/), [Podman](https://podman.io), [OrbStack](https://orbstack.dev) (macOS), [colima](https://github.com/abiosoft/colima), [nerdctl](https://github.com/containerd/nerdctl)

## Cloning

This repo uses git submodules. Clone with:

```bash
git clone --recurse-submodules https://github.com/vaporif/mercury.git
```

To auto-pull submodules on future `git pull`/`git checkout`:

```bash
git config submodule.recurse true
```

## Building and Testing

```bash
cargo build
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

### Just recipes

A `justfile` wraps the common commands:

```bash
just check        # Run all checks (typos, TOML lint, fmt, clippy, test)
just test         # Run unit tests
just clippy       # Run clippy
just fmt          # Format check
just fmt-fix      # Auto-format everything (Rust, TOML, Nix)
just e2e          # Run E2E tests (requires Docker)
just check-typos  # Spell check
just check-toml   # TOML lint
```

### E2E tests

E2E tests use Docker to run local chain instances:

```bash
just e2e
# or directly:
cargo nextest run -p mercury-e2e --run-ignored all
```

## CI

CI runs on every push to `main` and on pull requests. It includes:

- **Check** — clippy, tests, format check
- **Cross** — cross-compilation for linux (x86_64/aarch64 musl) and macOS (x86_64/aarch64)
- **Lint** — typos, TOML lint, gitleaks
- **Nix** — nix fmt, flake check, nix build
- **Deny** — `cargo-deny` for dependency auditing (licenses, vulnerabilities)
- **E2E** — end-to-end relay tests with Docker

## Code Style

### Rust

- **Edition 2024**, MSRV `1.91.0`
- Formatted with `cargo fmt` — non-negotiable
- Clippy with `all`, `pedantic`, `nursery`, and `cargo` lint groups enabled — zero warnings (`-D warnings`). Suppress individual lints with `#[allow]` and a justification comment
- Modern module syntax: `foo.rs` + `foo/bar.rs` — never `foo/mod.rs`
- Prefer `impl Trait` in args/returns over `Box<dyn Trait>` where possible
- Use iterators and combinators (`.map`, `.filter`, `.collect`) over manual loops
- `eyre` for error handling with retryability tracking — propagate with `?`, no `.unwrap()` or `.expect()` in production code
- `tracing` for structured logging — not `println!` or `log`
- `async-trait` for async trait methods

### Formatting

- TOML — `taplo`
- Nix — `alejandra`

### AI-Assisted Contributions

AI assistants are welcome as tools. The human contributor bears full responsibility for every line submitted — correctness, licensing, and review. If you used AI to generate code, you must have read and verified it yourself before opening a PR. Unreviewed AI output will be declined.

## Understanding the Codebase

Read the [Architecture](./docs/architecture.md) doc before diving into the code. It covers the trait hierarchy, worker pipeline, crate boundaries, and design decisions.

### Crate Map

| Crate | Description |
|-------|-------------|
| `mercury-relayer` (`crates/cli`) | CLI binary — entry point, config parsing, worker orchestration |
| `mercury-cosmos` (`crates/chains/cosmos`) | Cosmos chain implementation — RPC, protobuf, tx signing |
| `mercury-relay` (`crates/relay`) | Worker pipeline, generic over chain traits |
| `mercury-chain-traits` (`crates/chain-traits`) | Chain types, messaging, queries, relay traits (~16 traits) |
| `mercury-core` (`crates/core`) | Error types, encoding, worker trait |
| `mercury-e2e` (`crates/e2e`) | End-to-end tests |

### Entry Points

- **Adding a chain?** Start with [Adding a new chain](./docs/adding-a-chain.md) and use `crates/chains/cosmos/` as reference
- **Understanding the relay pipeline?** Read `crates/relay/src/workers/` — each worker is a self-contained module
- **Working on traits?** All chain abstractions live in `crates/chain-traits/src/`

### Design Principles

- **Plain traits, no frameworks.** Direct `impl` blocks, no provider indirection or macro-heavy abstractions
- **Few, focused traits.** ~16 traits grouped by concern — `ChainTypes` and `IbcTypes<C>` carry associated types, not one trait per type
- **Concrete error type.** One `eyre`-based error with retryability tracking, no generic error parameters
- **Don't abstract implementation details.** Transaction internals (fees, nonces, signing) are concrete methods on chain types, not traits
