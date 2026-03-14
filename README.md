# Mercury

[![CI](https://github.com/vaporif/mercury/actions/workflows/check.yml/badge.svg)](https://github.com/vaporif/mercury/actions/workflows/check.yml)
[![E2E](https://github.com/vaporif/mercury/actions/workflows/e2e.yml/badge.svg)](https://github.com/vaporif/mercury/actions/workflows/e2e.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

An IBC v2 relayer in Rust. Plain traits, no frameworks.

Mercury relays packets between IBC-connected blockchains. Unlike [hermes-sdk](https://github.com/informalsystems/hermes-sdk), which uses Context-Generic Programming (250+ component traits, macro-heavy abstractions, 20-minute compile times), Mercury uses plain Rust traits and generics — ~16 focused traits, readable error messages, and standard tooling that just works. If you've tried to contribute to an IBC relayer and bounced off the complexity, this is for you.

## Status

Early active development. Core IBC v2 relay pipeline and Docker base transfer E2E test for Cosmos-to-Cosmos relaying. Not yet tested against live chains — use at your own risk.

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
| `mercury-relayer` | CLI binary — `mercury-relayer start`, `mercury-relayer status` |
| `mercury-cosmos` | Cosmos chain implementation — RPC, protobuf, tx signing |
| `mercury-relay` | Worker pipeline, generic over chain traits |
| `mercury-chain-traits` | Chain types, messaging, queries, relay traits |
| `mercury-core` | Error types, encoding, worker trait |

## Docs

- [Why rewrite?](./docs/why-rewrite.md)
- [IBC v2](./docs/ibc-v2.md)
- [Adding a new chain](./docs/adding-a-chain.md)

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
