# Feature Parity: Mercury vs Hermes

Comparison of Mercury (IBC v2) against [Hermes](https://github.com/informalsystems/hermes) (IBC v1). Excludes IBC v1-only features (connection/channel handshakes, channel upgrades) and chains requiring IBC v1 migration (Namada, Penumbra).

| Feature | Hermes | Mercury | Priority |
|---|---|---|---|
| **Chain Support** | | | |
| Cosmos SDK | Yes | Yes | -- |
| Ethereum/EVM | No | Yes | -- |
| Solana | No | Planned | P1 |
| **Packet Relay** | | | |
| Packet relay (recv/ack/timeout) | Yes | Yes | -- |
| Periodic packet sweeping | Yes | Yes | -- |
| Packet filtering (allow/deny) | Yes | Yes | -- |
| Packet clearing (manual CLI) | Yes | Yes | -- |
| Packet sequence exclusions | Yes | Planned | P3 |
| ICS20 memo/receiver size filtering | Yes | Planned | P3 |
| Clear on start | Yes | Planned | P2 |
| Clear limit (max packets per sweep) | Yes | Planned | P2 |
| Tx confirmation tracking | Yes | Planned | P2 |
| **Client Management** | | | |
| Create client | Yes | Yes | -- |
| Update client (periodic refresh) | Yes | Yes | -- |
| Client upgrade (chain upgrade) | Yes | Yes | -- |
| Misbehaviour detection | Yes | Yes | -- |
| Misbehaviour submission | Yes | Yes | -- |
| Misbehaviour CLI command | Yes | Planned | P2 |
| Configurable client refresh rate | Yes | Planned | P3 |
| **Fee & Gas** | | | |
| Static gas pricing | Yes | Yes | -- |
| Dynamic gas pricing | Yes | Yes | -- |
| Gas multiplier | Yes | Yes | -- |
| Fee granter | Yes | Yes | -- |
| Max gas / default gas | Yes | Yes | -- |
| **Key Management & Signing** | | | |
| File-based keys | Yes | Yes | -- |
| HSM support | No | Planned | P2 |
| **Configuration** | | | |
| Memo prefix/overwrite | Yes | Planned | P3 |
| Trusted node mode | Yes | Planned | P3 |
| Configurable retry params | Yes | Planned | P3 |
| Sequential batch tx mode | Yes | Planned | P3 |
| Ethermint address derivation | Yes | Planned | P3 |
| CometBFT compat mode (0.34/0.37) | Yes | Planned | P3 |
| Extension options (Ethermint dynamic fee) | Yes | Planned | P3 |
| Per-chain clear interval override | Yes | Planned | P3 |
| Idle worker auto-cleanup | Yes | Planned | P3 |
| Event source: WebSocket push | Yes | Yes | -- |
| Event source: RPC polling | Yes | Yes | -- |
| gRPC TLS | Yes | Yes | -- |
| `config validate` | Yes | Yes | -- |
| `config auto` (chain registry) | Yes | N/A | -- |
| **Middleware** | | | |
| Fee Middleware (ICS 29) | Yes | Planned | P2 |
| Interchain Accounts (ICS 27) | Yes | Planned | P2 |
| Cross-Chain Queries (ICS 31) | Yes | Planned | P3 |
| **Telemetry & Observability** | | | |
| Prometheus/OTLP metrics | Yes | Yes | -- |
| TX latency / gas histograms | Yes | Yes | -- |
| Health check endpoint | Yes | Yes | -- |
| Balance monitoring/alerts | Yes | Planned | P2 |
| REST API | Yes | Planned | P2 |
| Custom telemetry histogram buckets | Yes | Planned | P3 |
| Runtime log level control | Yes | Planned | P3 |
| **CLI Commands** | | | |
| `start` / `status` / `health-check` | Yes | Yes | -- |
| `create client` / `update client` | Yes | Yes | -- |
| `query client state` | Yes | Yes | -- |
| `query packet commitments` | Yes | Yes | -- |
| `query packet commitment` (single) | Yes | Planned | P2 |
| `query packet ack` (single) | Yes | Planned | P2 |
| `query packet acks` | Yes | Planned | P2 |
| `query packet pending` | Yes | Planned | P2 |
| `query packet pending-sends` | Yes | Planned | P2 |
| `query packet pending-acks` | Yes | Planned | P2 |
| `keys add/delete/list/balance` | Yes | Planned | P1 |
| `clear packets` | Yes | Yes | -- |
| `tx transfer` (initiate ICS20 transfer) | Yes | Planned | P3 |
| `query tx events` | Yes | Planned | P3 |
| `query denom trace` | Yes | Planned | P3 |
| `evidence` (Tendermint evidence) | Yes | Planned | P3 |
| `listen` (event display) | Yes | Planned | P3 |
| Shell completions | Yes | Planned | P3 |
| **Caching** | | | |
| RPC/state caching | Yes | N/A | -- |

**Priority legend:**
- **P1** -- Core operational needs
- **P2** -- Important for production use
- **P3** -- Nice to have

## N/A notes

**Caching** -- Hermes needs heavy caching because its sync, thread-per-channel model ends up hitting the same RPCs repeatedly. Mercury's async pipeline doesn't have this problem -- workers fetch what they need and pass it downstream through channels. We still cache where it makes sense (status TTL, bounded client/consensus state cache, TX nonce coordinator), just not as a workaround for the architecture.

**`config auto`** -- Hermes pulls chain config from the Cosmos Chain Registry. Mercury talks to Ethereum and Solana too, so the Cosmos registry alone doesn't cut it.
