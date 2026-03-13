# Mercury Roadmap

Production readiness tasks ordered by priority. Each item is scoped as an independent unit of work.

---

## Completed

### ~~1. Packet Clearing / Flushing~~

Implemented in `clearing_worker.rs`. Periodically scans source chain for unrelayed packet commitments, cross-references against destination receipts, and feeds recovered `SendPacket` events into the event pipeline. Enabled via `clearing_interval` config.

---

### ~~5. Misbehaviour Detection~~

Implemented in `misbehaviour_worker.rs`. Incrementally scans consensus state heights, verifies update headers against the source chain. On detection, submits `MsgSubmitMisbehaviour` and terminates the relay. Enabled via `misbehaviour_scan_interval` config.

---

### ~~4. Gas Estimation / Dynamic Fees~~

Simulation-based gas estimation with dynamic pricing support. `cosmos.tx.v1beta1.Service/Simulate` estimates gas per batch, with configurable multiplier (default 1.3) and optional max cap. Dynamic gas price resolution auto-detects osmosis txfees or skip feemarket backends via gRPC probes, caches the result with `OnceLock`. Message batches split by `max_msg_num` and `max_tx_size`, with parallel submission (semaphore-bounded, max 3 concurrent). Fee granter passthrough supported. Falls back to static price and default gas on simulation failure.

Config: `gas_multiplier`, `max_gas`, `default_gas`, `fee_granter`, `dynamic_gas_price`, `max_tx_size` (all optional).

---

## High Priority

### 2. Prometheus Metrics

Expose a Prometheus `/metrics` endpoint with operational metrics. Operators need dashboards for alerting and capacity planning.

**Scope:**
- Add `metrics` and `prometheus` crates
- Instrument: packets relayed (counter, by direction), tx submissions (counter, success/fail), relay latency (histogram), queue depth (gauge), client expiry countdown (gauge), RPC query latency (histogram), consecutive failures (gauge)
- Expose HTTP `/metrics` endpoint on configurable port (reuse or extend health endpoint)
- Add `metrics_port: Option<u16>` to config

---

### ~~3. Packet Filtering~~

Implemented in `filter.rs` in `mercury-relay`. Configurable allow/deny policies with glob patterns on payload source ports. Filter applied in EventWatcher and ClearingWorker before events reach PacketWorker. Config: `[relays.packet_filter]` with `policy` and `source_ports` fields.

---

## Medium Priority

### 6. CLI Query Commands

Add query subcommands for debugging and operational inspection. Currently the only introspection is `status`.

**Scope:**
- `query client-state --chain <id> --client <id>` — show client state (latest height, trusting period, frozen status)
- `query consensus-state --chain <id> --client <id> --height <h>` — show consensus state at height
- `query packet-commitments --chain <id> --client <id>` — list outstanding packet commitments
- `query unreceived-packets --chain <id> --client <id>` — cross-reference commitments vs receipts
- Output as JSON for scriptability

---

### 7. Client Upgrade

When a chain undergoes a software upgrade, its light clients on counterparty chains need upgrading too. Without this, the relay stops after a chain upgrade.

**Scope:**
- Add `UpgradeClientPayloadBuilder` and `UpgradeClientMessageBuilder` to chain traits
- Cosmos implementation: query upgrade plan from chain, build `MsgUpgradeClient`
- Detection: watch for `upgrade/plan` events or poll upgrade plan query
- Can be a manual CLI command first (`upgrade client --chain <id> --client <id>`), automated detection later

---

### 8. Key Management CLI

Currently key files must be manually created and referenced in TOML config. Add CLI commands for key lifecycle.

**Scope:**
- `keys add --chain <id> --name <name>` — generate new key, save to key file
- `keys add --chain <id> --name <name> --recover` — recover from mnemonic
- `keys delete --chain <id> --name <name>` — remove key file
- `keys list --chain <id>` — list available keys with addresses
- `keys show --chain <id> --name <name>` — show address and public key
- Store keys in `~/.mercury/keys/<chain_id>/` by default

---

### 9. Configurable Retry / Backoff

Currently hardcoded: 25 max consecutive failures, 1s-30s exponential backoff. Operators need to tune these for their environment.

**Scope:**
- Config per relay:
  ```toml
  [relays.retry]
  max_consecutive_failures = 25
  initial_backoff_secs = 1
  max_backoff_secs = 30
  ```
- Pass retry config through to `TxWorker` and `SrcTxWorker`
- Defaults match current hardcoded values (backward compatible)

---

### 10. Graceful Shutdown

`spawn_relay_pair` in the CLI creates `CancellationToken`s inside the spawned task with no external handle. On Ctrl+C, relay tasks are aborted rather than drained — in-flight transactions may be lost or duplicated.

**Scope:**
- Create `CancellationToken` per relay pair in `run_start`, pass into `spawn_relay_pair`
- On Ctrl+C, cancel all tokens and `await` join handles with a drain timeout
- `TxWorker`: on cancellation, finish submitting the current batch before exiting
- Config: `shutdown_drain_secs: Option<u64>` (default 10)

---

## Lower Priority

### 11. REST / gRPC API

Expose a management API for remote monitoring and control beyond Prometheus metrics.

**Scope:**
- REST endpoints: `GET /status`, `GET /relays`, `GET /relay/:id/packets`, `POST /relay/:id/clear`
- Optional: gRPC reflection for programmatic access
- Config: `api_port: Option<u16>`
- Consider axum or tonic for implementation

---

### 12. Multi-Chain Relay

Support relaying across >2 chains without requiring N^2 relay config entries. Currently each pair needs an explicit `[[relays]]` block.

**Scope:**
- Auto-discovery: given a set of chains, discover existing clients and relay paths
- Or: simplified config that generates relay pairs from a chain group
- Requires: client query infrastructure from task #6

---

### 13. IBC Incentivized Packet Fee Filtering

Allow operators to filter packets by minimum relay fees, only relaying packets that offer sufficient incentives. Blocked on IBC v2 fee incentivization — the v1 fee module (`ibc.applications.fee.v1`) uses port/channel identifiers which don't exist in IBC v2. Once a v2 fee mechanism exists, implement:

**Scope:**
- `IncentivizedPacketQuery` trait in `chain-traits` for querying packet fees
- Fee filter config per relay (keyed by client ID in v2)
- Filter applied in `PacketWorker` before building recv messages
- ANY-of-ANY matching semantics (matching hermes behavior)

---

### 14. Memo Support

Allow operators to set a custom memo field on relayed packets for attribution and analytics.

**Scope:**
- Config: `memo: Option<String>` per relay
- Pass memo through to `MsgRecvPacket`, `MsgAcknowledgePacket`, `MsgTimeout`
- Default: empty or `"mercury/<version>"`

