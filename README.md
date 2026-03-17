# Mercury IBC v2 Relayer

[![Cosmos ecosystem][cosmos-shield]][cosmos-link]

[![CI](https://github.com/vaporif/mercury/actions/workflows/ci.yml/badge.svg)](https://github.com/vaporif/mercury/actions/workflows/ci.yml)
[![E2E](https://github.com/vaporif/mercury/actions/workflows/e2e.yml/badge.svg)](https://github.com/vaporif/mercury/actions/workflows/e2e.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

<!-- [![Nix](https://github.com/vaporif/mercury/actions/workflows/nix.yml/badge.svg)](https://github.com/vaporif/mercury/actions/workflows/nix.yml) -->

A cross-chain IBC v2 relayer in Rust. Plain traits, no frameworks.

Mercury relays packets between IBC-connected blockchains, including across fundamentally different chain types. Cosmos↔Cosmos works today; Cosmos→EVM is in progress. Unlike [hermes](https://github.com/informalsystems/hermes) (primarily Cosmos/Tendermint chains, sync architecture) and [hermes-sdk](https://github.com/informalsystems/hermes-sdk) (correct cross-chain approach buried under 250+ component traits), Mercury shares all relay logic across chain pairs through ~21 plain Rust traits with a wrapper pattern for orphan rule avoidance.

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
| `mercury-cosmos-counterparties` | Cosmos — supported counterparties (currently: Ethereum) |
| `mercury-ethereum-counterparties` | Ethereum — supported counterparties (currently: Cosmos) |
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

[cosmos-shield]: https://img.shields.io/static/v1?label=&labelColor=1B1E36&color=1B1E36&message=cosmos%20ecosystem&style=for-the-badge&logo=data:image/svg+xml;base64,PD94bWwgdmVyc2lvbj0iMS4wIiBlbmNvZGluZz0idXRmLTgiPz4KPCEtLSBHZW5lcmF0b3I6IEFkb2JlIElsbHVzdHJhdG9yIDI0LjMuMCwgU1ZHIEV4cG9ydCBQbHVnLUluIC4gU1ZHIFZlcnNpb246IDYuMDAgQnVpbGQgMCkgIC0tPgo8c3ZnIHZlcnNpb249IjEuMSIgaWQ9IkxheWVyXzEiIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyIgeG1sbnM6eGxpbms9Imh0dHA6Ly93d3cudzMub3JnLzE5OTkveGxpbmsiIHg9IjBweCIgeT0iMHB4IgoJIHZpZXdCb3g9IjAgMCAyNTAwIDI1MDAiIHN0eWxlPSJlbmFibGUtYmFja2dyb3VuZDpuZXcgMCAwIDI1MDAgMjUwMDsiIHhtbDpzcGFjZT0icHJlc2VydmUiPgo8c3R5bGUgdHlwZT0idGV4dC9jc3MiPgoJLnN0MHtmaWxsOiM2RjczOTA7fQoJLnN0MXtmaWxsOiNCN0I5Qzg7fQo8L3N0eWxlPgo8cGF0aCBjbGFzcz0ic3QwIiBkPSJNMTI1Mi42LDE1OS41Yy0xMzQuOSwwLTI0NC4zLDQ4OS40LTI0NC4zLDEwOTMuMXMxMDkuNCwxMDkzLjEsMjQ0LjMsMTA5My4xczI0NC4zLTQ4OS40LDI0NC4zLTEwOTMuMQoJUzEzODcuNSwxNTkuNSwxMjUyLjYsMTU5LjV6IE0xMjY5LjQsMjI4NGMtMTUuNCwyMC42LTMwLjksNS4xLTMwLjksNS4xYy02Mi4xLTcyLTkzLjItMjA1LjgtOTMuMi0yMDUuOAoJYy0xMDguNy0zNDkuOC04Mi44LTExMDAuOC04Mi44LTExMDAuOGM1MS4xLTU5Ni4yLDE0NC03MzcuMSwxNzUuNi03NjguNGM2LjctNi42LDE3LjEtNy40LDI0LjctMmM0NS45LDMyLjUsODQuNCwxNjguNSw4NC40LDE2OC41CgljMTEzLjYsNDIxLjgsMTAzLjMsODE3LjksMTAzLjMsODE3LjljMTAuMywzNDQuNy01Ni45LDczMC41LTU2LjksNzMwLjVDMTM0MS45LDIyMjIuMiwxMjY5LjQsMjI4NCwxMjY5LjQsMjI4NHoiLz4KPHBhdGggY2xhc3M9InN0MCIgZD0iTTIyMDAuNyw3MDguNmMtNjcuMi0xMTcuMS01NDYuMSwzMS42LTEwNzAsMzMycy04OTMuNSw2MzguOS04MjYuMyw3NTUuOXM1NDYuMS0zMS42LDEwNzAtMzMyCglTMjI2Ny44LDgyNS42LDIyMDAuNyw3MDguNkwyMjAwLjcsNzA4LjZ6IE0zNjYuNCwxNzgwLjRjLTI1LjctMy4yLTE5LjktMjQuNC0xOS45LTI0LjRjMzEuNi04OS43LDEzMi0xODMuMiwxMzItMTgzLjIKCWMyNDkuNC0yNjguNCw5MTMuOC02MTkuNyw5MTMuOC02MTkuN2M1NDIuNS0yNTIuNCw3MTEuMS0yNDEuOCw3NTMuOC0yMzBjOS4xLDIuNSwxNSwxMS4yLDE0LDIwLjZjLTUuMSw1Ni0xMDQuMiwxNTctMTA0LjIsMTU3CgljLTMwOS4xLDMwOC42LTY1Ny44LDQ5Ni44LTY1Ny44LDQ5Ni44Yy0yOTMuOCwxODAuNS02NjEuOSwzMTQuMS02NjEuOSwzMTQuMUM0NTYsMTgxMi42LDM2Ni40LDE3ODAuNCwzNjYuNCwxNzgwLjRMMzY2LjQsMTc4MC40CglMMzY2LjQsMTc4MC40eiIvPgo8cGF0aCBjbGFzcz0ic3QwIiBkPSJNMjE5OC40LDE4MDAuNGM2Ny43LTExNi44LTMwMC45LTQ1Ni44LTgyMy03NTkuNVMzNzQuNCw1ODcuOCwzMDYuOCw3MDQuN3MzMDAuOSw0NTYuOCw4MjMuMyw3NTkuNQoJUzIxMzAuNywxOTE3LjQsMjE5OC40LDE4MDAuNHogTTM1MS42LDc0OS44Yy0xMC0yMy43LDExLjEtMjkuNCwxMS4xLTI5LjRjOTMuNS0xNy42LDIyNC43LDIyLjYsMjI0LjcsMjIuNgoJYzM1Ny4yLDgxLjMsOTk0LDQ4MC4yLDk5NCw0ODAuMmM0OTAuMywzNDMuMSw1NjUuNSw0OTQuMiw1NzYuOCw1MzcuMWMyLjQsOS4xLTIuMiwxOC42LTEwLjcsMjIuNGMtNTEuMSwyMy40LTE4OC4xLTExLjUtMTg4LjEtMTEuNQoJYy00MjIuMS0xMTMuMi03NTkuNi0zMjAuNS03NTkuNi0zMjAuNWMtMzAzLjMtMTYzLjYtNjAzLjItNDE1LjMtNjAzLjItNDE1LjNjLTIyNy45LTE5MS45LTI0NS0yODUuNC0yNDUtMjg1LjRMMzUxLjYsNzQ5Ljh6Ii8+CjxjaXJjbGUgY2xhc3M9InN0MSIgY3g9IjEyNTAiIGN5PSIxMjUwIiByPSIxMjguNiIvPgo8ZWxsaXBzZSBjbGFzcz0ic3QxIiBjeD0iMTc3Ny4zIiBjeT0iNzU2LjIiIHJ4PSI3NC42IiByeT0iNzcuMiIvPgo8ZWxsaXBzZSBjbGFzcz0ic3QxIiBjeD0iNTUzIiBjeT0iMTAxOC41IiByeD0iNzQuNiIgcnk9Ijc3LjIiLz4KPGVsbGlwc2UgY2xhc3M9InN0MSIgY3g9IjEwOTguMiIgY3k9IjE5NjUiIHJ4PSI3NC42IiByeT0iNzcuMiIvPgo8L3N2Zz4K
[cosmos-link]: https://cosmos.network
