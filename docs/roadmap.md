# Mercury Roadmap

Production readiness tasks ordered by priority. Each item is scoped as an independent unit of work.

---

## Critical — Blocking Production

### EVM→Cosmos Proving

Wasm light client never advances past height {0,0} in tests. Without this, Ethereum→Cosmos direction is non-functional.

**Scope:**
- Fix mock proving so wasm client advances height in E2E tests
- Real SP1 proving returns dummy proofs — integrate actual circuit validation
- Verify full roundtrip: Cosmos→EVM→Cosmos with real proofs

---

### Protobuf Client Message Wrapping

`MsgUpdateClient.client_message` currently sends raw bytes instead of properly wrapped `ClientMessage` proto. Cosmos chains reject malformed updates.

**Scope:**
- Wrap update payloads in proper `Any`-typed `ClientMessage` proto
- Verify against ibc-go expectations for both Tendermint and Wasm client types

---

## High Priority

### Packet Clearing Recovery Hardening

`PacketSweeper` exists but is never exercised in tests. This is the safety net for stuck packets.

**Scope:**
- E2E test: stop relay mid-flight, accumulate packets, restart, verify all delivered
- E2E test: inject missed events (simulate RPC blip), verify sweeper recovers them
- Verify sweeper correctly cross-references commitments vs receipts on both sides

---

### WebSocket Event Source

Currently only RPC polling. WebSocket push gives lower latency packet detection.

**Scope:**
- Add WebSocket event subscription alongside existing RPC polling in EventWatcher
- Config per chain: `event_source = "rpc" | "websocket"`
- Fallback: if WebSocket disconnects, degrade to RPC polling automatically
- Reconnection with exponential backoff

---

### Gas Estimation Improvements

Hermes has dynamic gas pricing (query chain for current rates), simulation multipliers, and fee capping. Mercury has basics but needs hardening.

**Scope:**
- Dynamic gas price queries (EIP-1559 style for EVM, feemarket module for Cosmos)
- Configurable simulation gas multiplier per chain
- Max fee cap to prevent surprise costs
- Fallback gas when simulation fails (configurable `default_gas`)

---

### Pending Transaction Tracking

No tracking of submitted-but-unconfirmed txs. If a tx gets stuck, the relayer may resubmit duplicates or lose track.

**Scope:**
- Track pending txs with height-based expiration (tx not confirmed after N blocks → retry)
- Prevent duplicate submission of same messages while a prior tx is still pending
- Expose pending tx count in metrics

---

### Misbehaviour Detection Testing

Detector and message builder are implemented but untested — no forked chain or injected headers in E2E.

**Scope:**
- E2E test: inject conflicting header, verify misbehaviour detected and submitted
- E2E test: verify relay halts after misbehaviour submission
- Test metric emission for `misbehaviour_detected` / `misbehaviour_submitted`

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

### Packet Filter Testing

Config for allow/deny by port exists but no test validates it works.

**Scope:**
- E2E test: configure deny filter, send packet on denied port, verify not relayed
- E2E test: configure allow filter, verify only matching ports relayed
- Test regex/glob pattern matching on port names

---

### Bidirectional Concurrent Relay Testing

No test for simultaneous A→B and B→A traffic. Real networks have bidirectional packet flow.

**Scope:**
- E2E test: send packets in both directions concurrently using `JoinSet`
- Verify no deadlocks, message ordering preserved per-direction
- Stress test: 50+ concurrent packets across both directions

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

---

### Chain Registry Integration

Reduce operator config burden by fetching chain metadata from a registry.

**Scope:**
- Fetch RPC/gRPC endpoints, gas prices, chain IDs from cosmos/chain-registry
- `mercury init --chain cosmoshub-4` generates config block from registry
- Periodic refresh of gas prices from registry or on-chain query

---

### Height-Based Timeout Support

Only timestamp timeouts are tested. Height-based timeouts are part of the IBC spec.

**Scope:**
- E2E test: send packet with height-based timeout, verify timeout processed
- Verify height comparison logic handles revision numbers correctly

---

### Chain Downtime Recovery

No test for destination chain going down and coming back.

**Scope:**
- E2E test: pause destination chain, send packets, resume, verify all delivered
- Verify EventWatcher tolerates RPC unavailability without crashing
- Verify backoff and reconnection behavior
