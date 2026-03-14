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

- `cargo-nextest` — test runner for E2E tests
- `taplo` — TOML formatter/linter
- `typos` — spell checker

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

### AI-Generated Code

AI assistants are welcome as tools, not as authors. Review what they produce — if you haven't read and verified the code yourself, don't submit it. PRs with obvious unreviewed AI output will be declined.

## Project Structure

| Crate | Description |
|-------|-------------|
| `mercury-relayer` (`crates/cli`) | CLI binary |
| `mercury-cosmos` (`crates/chains/cosmos`) | Cosmos chain implementation |
| `mercury-relay` (`crates/relay`) | Worker pipeline, generic over chain traits |
| `mercury-chain-traits` (`crates/chain-traits`) | Chain types, messaging, queries, relay traits |
| `mercury-core` (`crates/core`) | Error types, encoding, worker trait |
| `mercury-e2e` (`crates/e2e`) | End-to-end tests |

See [Architecture](./docs/architecture.md) for the full pipeline, trait hierarchy, and design decisions.

## Adding a New Chain

See [Adding a new chain](./docs/adding-a-chain.md) for a step-by-step guide.
