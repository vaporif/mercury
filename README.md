# Mercury

[![CI](https://github.com/vaporif/mercury/actions/workflows/check.yml/badge.svg)](https://github.com/vaporif/mercury/actions/workflows/check.yml)
[![Nix](https://github.com/vaporif/mercury/actions/workflows/nix.yml/badge.svg)](https://github.com/vaporif/mercury/actions/workflows/nix.yml)
[![E2E](https://github.com/vaporif/mercury/actions/workflows/e2e.yml/badge.svg)](https://github.com/vaporif/mercury/actions/workflows/e2e.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

A cross-chain IBC v2 relayer in Rust. Plain traits, no frameworks.

Mercury relays packets between IBC-connected blockchains, including across fundamentally different chain types. Cosmos↔Cosmos works today; Cosmos→EVM is in progress. Unlike [hermes](https://github.com/informalsystems/hermes) (Cosmos-only, sync architecture) and [hermes-sdk](https://github.com/informalsystems/hermes-sdk) (correct cross-chain approach buried under 250+ component traits), Mercury shares all relay logic across chain pairs through ~21 plain Rust traits with a wrapper pattern for orphan rule avoidance.

## Status

Early active development. Core IBC v2 relay pipeline with Cosmos↔Cosmos packet relay working in E2E tests. Cosmos→EVM cross-chain relay in progress. Not yet tested against live chains — use at your own risk.

## How it works

```
EventWatcher    ─┐
                 ├→ PacketWorker → TxWorker (dst chain)
ClearingWorker  ─┘       ↓
                    SrcTxWorker (src chain)

ClientRefreshWorker → TxWorker (dst chain)
MisbehaviourWorker (independent, cancels relay on detection)
```

Each relay direction (A→B, B→A) runs its own set of workers connected by `tokio::mpsc` channels. See [Architecture](./docs/architecture.md) for the full pipeline, crate layout, and trait hierarchy.

## Crates

| Crate | Description |
|-------|-------------|
| `mercury-cli` | CLI binary — `mercury-relayer start`, `mercury-relayer status` |
| `mercury-cosmos` | Cosmos chain — RPC, protobuf, tx signing |
| `mercury-ethereum` | EVM chain — alloy, ICS07 contract interaction |
| `mercury-cosmos-bridges` | Cosmos wrapper — cross-chain impls (EVM→Cosmos via beacon) |
| `mercury-ethereum-bridges` | Ethereum wrapper — cross-chain impls (Cosmos→EVM) |
| `mercury-relay` | Worker pipeline, generic over chain traits |
| `mercury-chain-traits` | Chain types, messaging, queries, relay traits |
| `mercury-core` | Error types, encoding, worker trait, membership proofs |

## Docs

- [Why rewrite?](./docs/why-rewrite.md) — Hermes limitations, what CGP gets right, how Mercury applies the same insight without the framework
- [Architecture](./docs/architecture.md) — trait hierarchy, cross-chain design, crate layout, worker pipeline
- [IBC v2](./docs/ibc-v2.md) — Eureka protocol changes vs v1
- [Adding a new chain](./docs/adding-a-chain.md) — step-by-step guide

## Usage

```bash
# Start the relayer
mercury-relayer start --config relayer.toml

# Query chain status
mercury-relayer status --config relayer.toml --chain cosmoshub-4
```

See [`examples/relayer.toml`](./examples/relayer.toml) for a full config example.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) for development setup, building, testing, CI, and code style.

## License

Apache-2.0
