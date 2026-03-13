# Mercury Roadmap

Production readiness tasks ordered by priority. Each item is scoped as an independent unit of work.

---

## High Priority

### 1. Packet Clearing / Flushing

Periodically scan source and destination chains for unrelayed packet commitments and clear them. On startup (and at configurable intervals), query all outstanding `PacketCommitment`s on the source chain, check which lack a corresponding `PacketReceipt` or `Acknowledgement` on the destination, and relay the missing ones. This catches packets missed due to RPC blips, reorgs, or relay downtime — the lookback window only partially addresses this.

**Scope:**
- Add `PacketCommitmentQuery` to chain traits (query all commitments for a client)
- Add a `ClearingWorker` that runs on startup and periodically (configurable interval)
- Cross-reference source commitments against destination receipts/acks
- Feed missing packets into the existing `PacketWorker` pipeline
- Config: `clearing_interval_secs: Option<u64>` (None = startup only)

---

### 2. Prometheus Metrics

Expose a Prometheus `/metrics` endpoint with operational metrics. Operators need dashboards for alerting and capacity planning.

**Scope:**
- Add `metrics` and `prometheus` crates
- Instrument: packets relayed (counter, by direction), tx submissions (counter, success/fail), relay latency (histogram), queue depth (gauge), client expiry countdown (gauge), RPC query latency (histogram), consecutive failures (gauge)
- Expose HTTP `/metrics` endpoint on configurable port (reuse or extend health endpoint)
- Add `metrics_port: Option<u16>` to config

---

### 3. Packet Filtering

Allow operators to control which packets the relayer processes. Production relayers shouldn't relay everything on a chain.

**Scope:**
- Config: allowlist/denylist by client ID
- Filter applied in `EventWatcher` before forwarding events to `PacketWorker`
- Default: relay everything (no filter = current behavior)
- Config structure:
  ```toml
  [relays.filter]
  policy = "allow" # or "deny"
  client_ids = ["07-tendermint-0"]
  ```

---

### 4. Gas Estimation / Dynamic Fees

Replace static gas price with simulation-based estimation. Static pricing overpays on quiet chains and fails on fee-market chains (EIP-1559 style or Cosmos SDK fee market module).

**Scope:**
- Add `SimulateTx` method to chain traits — simulate message batch, return estimated gas
- Cosmos implementation: use `cosmos.tx.v1beta1.Service/Simulate` gRPC
- Apply configurable gas multiplier (default 1.1) for safety margin
- Fallback to configured static price if simulation fails
- Config: `gas_multiplier: Option<f64>`, `max_gas_price: Option<f64>`

---

### 5. Misbehaviour Detection

Detect and submit evidence when a light client is fed conflicting headers at the same height. This is a security-critical feature — without it, a compromised validator set can forge state proofs.

**Scope:**
- On each client update, compare the new header against the existing consensus state at that height
- If mismatch detected, build and submit `MsgSubmitMisbehaviour`
- Add `MisbehaviourBuilder` trait to chain traits
- Cosmos implementation: construct `MsgSubmitMisbehaviour` with two conflicting headers
- Log at error level and emit metric on detection

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

### 13. Memo Support

Allow operators to set a custom memo field on relayed packets for attribution and analytics.

**Scope:**
- Config: `memo: Option<String>` per relay
- Pass memo through to `MsgRecvPacket`, `MsgAcknowledgePacket`, `MsgTimeout`
- Default: empty or `"mercury/<version>"`

