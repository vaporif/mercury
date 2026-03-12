# Mercury

[![CI](https://github.com/vaporif/mercury/actions/workflows/check.yml/badge.svg)](https://github.com/vaporif/mercury/actions/workflows/check.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://www.rust-lang.org)

An IBC v2 relayer in Rust. Plain traits, no frameworks.

Mercury relays packets between IBC-connected blockchains. Unlike [hermes-sdk](https://github.com/informalsystems/hermes-sdk), which uses Context-Generic Programming (250+ component traits, macro-heavy abstractions, 20-minute compile times), Mercury uses plain Rust traits and generics — ~35 focused traits, readable error messages, and standard tooling that just works. If you've tried to contribute to an IBC relayer and bounced off the complexity, this is for you.

## Status

Early active development. Core IBC v2 relay pipeline is implemented but **not yet tested against live chains**. No unit or integration tests yet. Use at your own risk. Or better don't even try to use until I add unit/integration/e2e tests.

## How it works

```
EventWatcher → PacketWorker → TxWorker (dst chain)
                    ↓
               SrcTxWorker (src chain)

ClientRefreshWorker → TxWorker (dst chain)
```

Each relay direction (A→B, B→A) runs its own set of workers connected by `tokio::mpsc` channels. See [Architecture](./docs/architecture.md) for the full pipeline, crate layout, and trait hierarchy.

## Crates

| Crate | Description |
|-------|-------------|
| `mercury-relayer` | CLI binary — `mercury-relayer start`, `mercury-relayer status` |
| `mercury-cosmos` | Cosmos chain implementation — RPC, protobuf, tx signing |
| `mercury-relay` | Worker pipeline, generic over chain traits |
| `mercury-chain-traits` | Chain types, messaging, queries, relay traits |
| `mercury-core` | Error types, encoding, worker trait |

## Docs

- [Why rewrite?](./docs/why-rewrite.md)
- [IBC v2](./docs/ibc-v2.md)

## Usage

```bash
# Start the relayer
mercury-relayer start --config relayer.toml

# Query chain status
mercury-relayer status --config relayer.toml --chain cosmoshub-4
```

See [`examples/relayer.toml`](./examples/relayer.toml) for a full config example.

## Development

### With Nix (recommended)

The project includes a Nix flake with a complete dev shell. If you have Nix with flakes enabled:

```bash
# Enter the dev shell (or use direnv with the included .envrc)
nix develop

# Build the binary via Nix
nix build
```

The dev shell provides the stable Rust toolchain (cargo, clippy, rustfmt, rust-analyzer), plus `cargo-nextest`, `taplo`, `typos`, and `actionlint`.

### Without Nix

Requires a stable Rust toolchain. Install via [rustup](https://rustup.rs).

### Building and testing

```bash
cargo build
cargo test
cargo clippy --workspace
```

## License

Apache-2.0
