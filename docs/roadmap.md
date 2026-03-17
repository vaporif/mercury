# Mercury Roadmap

Production readiness tasks ordered by priority. Each item is scoped as an independent unit of work.

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
