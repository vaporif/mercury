# Roadmap

What needs to happen before this is production-ready, roughly in priority order.

---

## Critical - blocking production

### EVM to Cosmos proving

The wasm light client never advances past height {0,0} in tests. Until this works, the entire Ethereum to Cosmos direction is dead.

- Fix mock proving so the wasm client actually advances height in E2E tests
- Real SP1 proving still returns dummy proofs - need actual circuit validation
- Verify full roundtrip: Cosmos to EVM to Cosmos with real proofs

---

### Protobuf client message wrapping

`MsgUpdateClient.client_message` sends raw bytes instead of a properly wrapped `ClientMessage` proto. Cosmos chains reject these.

- Wrap update payloads in proper `Any`-typed `ClientMessage` proto
- Verify against ibc-go expectations for both Tendermint and Wasm client types

---

## High priority

### Packet clearing recovery hardening

`PacketSweeper` exists but nothing tests it. It's the safety net for stuck packets and it has zero coverage.

- E2E test: stop relay mid-flight, accumulate packets, restart, verify all delivered
- E2E test: inject missed events (simulate RPC blip), verify sweeper recovers them
- Verify sweeper correctly cross-references commitments vs receipts on both sides

---

### WebSocket event source

Currently only RPC polling. WebSocket push gives lower latency packet detection.

- Add WebSocket event subscription alongside existing RPC polling in EventWatcher
- Config per chain: `event_source = "rpc" | "websocket"`
- Fallback: if WebSocket disconnects, degrade to RPC polling automatically
- Reconnection with exponential backoff

---

### Gas estimation improvements

Hermes has dynamic gas pricing, simulation multipliers, and fee capping. Mercury has the basics but needs work.

- Dynamic gas price queries (EIP-1559 style for EVM, feemarket module for Cosmos)
- Configurable simulation gas multiplier per chain
- Max fee cap to prevent surprise costs
- Fallback gas when simulation fails (configurable `default_gas`)

---

### Pending transaction tracking

No tracking of submitted-but-unconfirmed txs right now. If a tx gets stuck, the relayer can resubmit duplicates or lose track entirely.

- Track pending txs with height-based expiration (tx not confirmed after N blocks, retry)
- Prevent duplicate submission of same messages while a prior tx is still pending
- Expose pending tx count in metrics

---

### Misbehaviour detection testing

Detector and message builder are implemented but untested - no forked chain or injected headers in E2E.

- E2E test: inject conflicting header, verify misbehaviour detected and submitted
- E2E test: verify relay halts after misbehaviour submission
- Test metric emission for `misbehaviour_detected` / `misbehaviour_submitted`

---

## Medium priority

### CLI query commands

The command structure exists but implementations are all stubs. Need real query subcommands for debugging and operational inspection.

- `query client-state --chain <id> --client <id>` - show client state (latest height, frozen status)
- `query packet-commitments --chain <id> --client <id>` - list outstanding packet commitments
- `query unreceived-packets --chain <id> --client <id>` - cross-reference commitments vs receipts
- Output as JSON for scriptability

---

### Key management CLI

Key files must be manually created and referenced in TOML config. Like the query commands, the CLI structure exists but nothing is wired up.

- `keys add --chain <id> --name <name>` - generate new key, save to key file
- `keys add --chain <id> --name <name> --recover` - recover from mnemonic
- `keys delete --chain <id> --name <name>` - remove key file
- `keys list --chain <id>` - list available keys with addresses
- `keys balance --chain <id> --name <name>` - show balance
- Store keys in `~/.mercury/keys/<chain_id>/` by default

---

### Configurable retry / backoff

Hardcoded at 1s-60s exponential backoff. Operators will want to tune this.

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

### Packet filter testing

Config for allow/deny by port exists but no test validates it works.

- E2E test: configure deny filter, send packet on denied port, verify not relayed
- E2E test: configure allow filter, verify only matching ports relayed
- Test regex/glob pattern matching on port names

---

### Bidirectional concurrent relay testing

No test for simultaneous A to B and B to A traffic. Real networks have bidirectional packet flow.

- E2E test: send packets in both directions concurrently using `JoinSet`
- Verify no deadlocks, message ordering preserved per direction
- Stress test: 50+ concurrent packets across both directions

---

## Lower priority

### REST / gRPC API

Only a health endpoint exists. Need a proper management API for remote monitoring and control.

- REST endpoints: `GET /status`, `GET /relays`, `GET /relay/:id/packets`, `POST /relay/:id/clear`
- Optional: gRPC reflection for programmatic access
- Config: `api_port: Option<u16>`

---

### Multi-chain relay auto-discovery

Each relay pair currently needs an explicit `[[relays]]` block. With many chains this becomes N^2 config entries.

- Auto-discovery: given a set of chains, discover existing clients and relay paths
- Or: simplified config that generates relay pairs from a chain group
- Requires: client query infrastructure from CLI query commands task

---

### Chain registry integration

Operators shouldn't have to look up RPC endpoints and gas prices manually.

- Fetch RPC/gRPC endpoints, gas prices, chain IDs from cosmos/chain-registry
- `mercury init --chain cosmoshub-4` generates config block from registry
- Periodic refresh of gas prices from registry or on-chain query

---

### Height-based timeout support

Only timestamp timeouts are tested. Height-based timeouts are part of the IBC spec.

- E2E test: send packet with height-based timeout, verify timeout processed
- Verify height comparison logic handles revision numbers correctly

---

### Chain downtime recovery

No test for destination chain going down and coming back.

- E2E test: pause destination chain, send packets, resume, verify all delivered
- Verify EventWatcher tolerates RPC unavailability without crashing
- Verify backoff and reconnection behavior
