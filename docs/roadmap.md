# Mercury Roadmap

Production readiness tasks ordered by priority. Each item is scoped as an independent unit of work.

---

## Completed

### ~~Packet Clearing / Flushing~~

Implemented in `packet_sweeper.rs`. Periodically scans source chain for unrelayed packet commitments, cross-references against destination receipts, and feeds recovered `SendPacket` events into the event pipeline. Enabled via `sweep_interval` config.

---

### ~~Misbehaviour Detection~~

Implemented in `misbehaviour_worker.rs`. Incrementally scans consensus state heights, verifies update headers against the source chain. On detection, submits misbehaviour evidence and terminates the relay. Enabled via `misbehaviour_scan_interval` config.

---

### ~~Gas Estimation / Dynamic Fees~~

Simulation-based gas estimation with dynamic pricing support. Configurable multiplier (default 1.3) and optional max cap. Dynamic gas price resolution auto-detects osmosis txfees or skip feemarket backends via gRPC probes, caches the result with `OnceLock`. Message batches split by `max_msg_num` and `max_tx_size`, with parallel submission (semaphore-bounded, max 3 concurrent). Fee granter passthrough supported.

---

### ~~Packet Filtering~~

Implemented in `filter.rs`. Configurable allow/deny policies with glob patterns on payload source ports. Filter applied in EventWatcher and PacketSweeper before events reach PacketWorker. Config: `[relays.packet_filter]` with `policy` and `source_ports` fields.

---

### ~~Prometheus Metrics~~

Implemented in `crates/telemetry/`. Prometheus HTTP listener with configurable host/port. Metrics: TX latency (submitted/confirmed), query latency, gas paid, gas price, process metrics. Config: `metrics_port` and `metrics_host` in TOML.

---

### ~~Graceful Shutdown~~

`CancellationToken` per relay pair with signal handlers (SIGTERM, Ctrl+C). On shutdown, cancels all tokens and awaits join handles with a 30s drain timeout. All workers respect cancellation.

---

### ~~Ethereum (EVM) Chain Support~~

Full Ethereum/EVM integration via IBC v2 (Eureka) in `crates/chains/ethereum/`. Includes: types and queries (alloy primitives, contract ABIs), event parsing (SendPacket, WriteAck via log filters), message builders (ICS26Router calls), transaction submission (ECDSA signing, gas estimation, nonce management), SP1 prover integration (mock and Groth16 modes), and Cosmos↔Ethereum E2E relay tests. Plugin architecture registers Cosmos and Ethereum chains with relay pair plugins.

---

### ~~Coordinator~~

Transaction message coalescing via `TxCoordinatorHandle` in `crates/chain-cache/`. Queues messages from multiple concurrent callers and batches them before sending to chain, reducing transaction overhead.

---

### ~~Plugin Architecture~~

Registry-based chain and relay pair plugins in `crates/core/`. `ChainPlugin` trait defines chain type, connection, validation, and status query. `RelayPairPlugin` builds relay pairs from connected chains. Dynamic relay execution via `DynRelay`.

---

## Medium Priority

### CLI Query Commands

Add query subcommands for debugging and operational inspection. Command structure exists but implementations are stubbed.

**Scope:**
- `query client-state --chain <id> --client <id>` — show client state (latest height, frozen status)
- `query packet-commitments --chain <id> --client <id>` — list outstanding packet commitments
- `query unreceived-packets --chain <id> --client <id>` — cross-reference commitments vs receipts
- Output as JSON for scriptability

---

### Key Management CLI

Currently key files must be manually created and referenced in TOML config. Command structure exists but implementations are stubbed.

**Scope:**
- `keys add --chain <id> --name <name>` — generate new key, save to key file
- `keys add --chain <id> --name <name> --recover` — recover from mnemonic
- `keys delete --chain <id> --name <name>` — remove key file
- `keys list --chain <id>` — list available keys with addresses
- `keys balance --chain <id> --name <name>` — show balance
- Store keys in `~/.mercury/keys/<chain_id>/` by default

---

### Configurable Retry / Backoff

Currently hardcoded: 1s-60s exponential backoff. Operators need to tune these for their environment.

**Scope:**
- Config per relay:
  ```toml
  [relays.retry]
  max_consecutive_failures = 25
  initial_backoff_secs = 1
  max_backoff_secs = 60
  ```
- Pass retry config through to workers
- Defaults match current hardcoded values (backward compatible)

---

## Lower Priority

### REST / gRPC API

Expand beyond the existing health endpoint to a full management API for remote monitoring and control.

**Scope:**
- REST endpoints: `GET /status`, `GET /relays`, `GET /relay/:id/packets`, `POST /relay/:id/clear`
- Optional: gRPC reflection for programmatic access
- Config: `api_port: Option<u16>`

---

### Multi-Chain Relay Auto-Discovery

Currently each relay pair needs an explicit `[[relays]]` block. Support relaying across >2 chains without N^2 config entries.

**Scope:**
- Auto-discovery: given a set of chains, discover existing clients and relay paths
- Or: simplified config that generates relay pairs from a chain group
- Requires: client query infrastructure from CLI Query Commands task
