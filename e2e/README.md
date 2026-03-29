# Mercury E2E tests

End-to-end tests that spin up real chain infrastructure (Docker containers, Anvil, Kurtosis) and run the full relay pipeline.

## Running

```bash
# All E2E tests (requires Docker + Foundry for Ethereum tests)
cargo nextest run -p mercury-e2e --run-ignored all

# Cosmos-only tests (requires Docker)
cargo nextest run -p mercury-e2e --run-ignored all --test cosmos_cosmos

# Cosmos-Ethereum tests (requires Docker + Foundry)
cargo nextest run -p mercury-e2e --run-ignored all --test cosmos_ethereum
```

All tests are `#[ignore]` by default since they need external infrastructure.

## Cosmos <-> Cosmos

### Covered

| Test | What it validates |
|------|-------------------|
| `bootstrap_smoke` | Chain bootstrap: chain ID, RPC/gRPC endpoints, wallet setup |
| `binary_smoke` | CLI binary relay via subprocess with health check |
| `create_client_b_tracks_a` | CLI `create client` command: host=B, reference=A |
| `create_client_a_tracks_b` | CLI `create client` command: host=A, reference=B |
| `ibc_transfer` | Unidirectional A->B transfer with balance assertion |
| `bidirectional_transfer` | A->B then B->A roundtrip with native token un-escrowing |
| `packet_timeout` | 1s timeout -> relay detects timeout -> refund on source |
| `client_refresh_keeps_relay_alive` | Transfer after 10s idle gap (client refresh keeps light client alive) |
| `concurrent_transfers` | 5 transfers sent back-to-back, accumulated balance verified |

### Gaps

| Gap | Description | Priority |
|-----|-------------|----------|
| Clearing worker recovery | Missed packets recovered from on-chain state. Worker exists but never exercised. | High |
| Relay restart | Stop relay mid-flight, accumulate unrelayed packets, restart, verify delivery. | High |
| Misbehaviour detection | Detector + message builder implemented but untested. Needs forked chain or injected conflicting headers. | Medium |
| True concurrent sends | `concurrent_transfers` sends sequentially in a loop. Should use `JoinSet` for parallel submissions. | Medium |
| Large batch | 50+ packets to stress `TxWorker` batching (`max_msg_num` config). | Medium |
| Bidirectional concurrent | Simultaneous A->B and B->A traffic. | Medium |
| Packet filter | Config supports `packet_filter` allow/deny by port, but no test validates it. | Low |
| Multiple denominations | All tests use `"stake"` only. Should test a second denom for trace handling. | Low |
| Height-based timeout | Only timestamp timeout tested. | Low |
| Near-expiry timeout | Packet arriving just before timeout - verify delivery, not timeout. | Low |
| Chain downtime | Destination unreachable during relay, then recovers. | Low |

## Cosmos <-> Ethereum (Mock light client)

Tests using Anvil + Docker with mock/dummy light clients (no real beacon chain).

### Covered

| Test | What it validates |
|------|-------------------|
| `anvil_bootstrap_smoke` | Anvil bootstrap: contracts deployed (ICS26, ICS20, ERC20, mock verifier), chain ID valid |
| `context_setup_smoke` | Full cross-chain context setup: Cosmos + Anvil + client creation |
| `create_client_cosmos_host_eth_reference` | CLI `create client`: Ethereum client on Cosmos (wasm LC) |
| `create_client_eth_host_cosmos_reference` | CLI `create client`: Cosmos client on Ethereum (SP1 LC) |
| `eth_client_on_cosmos_advances_height` | Build mock update payload, submit to Cosmos, verify client height advances |
| `cosmos_to_eth_transfer` | Cosmos->Ethereum unidirectional transfer with balance assertion |
| `eth_to_cosmos_transfer` | Eth->Cosmos unidirectional transfer (seeds via Cosmos->Eth, then asserts return leg) |
| `cosmos_eth_roundtrip_transfer` | Cosmos->Eth->Cosmos full roundtrip with balance verification on both sides |

### Gaps

| Gap | Description | Priority |
|-----|-------------|----------|
| Packet timeout (Cosmos side) | No test for packets timing out when Cosmos is destination. | High |
| Packet timeout (Eth side) | No test for packets timing out on Ethereum. | Medium |
| Client refresh | No equivalent of `client_refresh_keeps_relay_alive` for Eth<->Cosmos. | Medium |
| Clearing worker recovery | Same gap as Cosmos<->Cosmos - never exercised. | Medium |
| Concurrent transfers | No parallel traffic test for Cosmos<->Eth. | Medium |
| Multiple denominations | Only `"stake"` tested. | Low |

## Cosmos <-> Ethereum (Beacon light client)

Tests using Kurtosis with a real beacon chain and beacon-based light client.

### Covered

| Test | What it validates |
|------|-------------------|
| `create_eth_client_on_cosmos_beacon` | Create real beacon-backed Ethereum client on Cosmos, verify non-zero initial height |
| `eth_client_on_cosmos_advances_height_beacon` | Build beacon update payload (waits for finality), submit to Cosmos, verify height advances |
| `cosmos_to_eth_transfer_beacon` | Cosmos->Ethereum unidirectional transfer via real beacon LC |
| `eth_to_cosmos_transfer_beacon` | Eth->Cosmos unidirectional transfer via beacon LC (seeds via Cosmos->Eth, then asserts return leg) |
| `cosmos_eth_roundtrip_transfer_beacon` | Full roundtrip Cosmos->Eth->Cosmos via beacon LC (handles sync committee period crossings) |

### Gaps

| Gap | Description | Priority |
|-----|-------------|----------|
| Client refresh | No long-idle-then-transfer test for beacon LC. | Medium |
| Concurrent transfers | No parallel traffic test with beacon LC. | Low |
| Sync committee period crossing | Roundtrip test may cross periods but no dedicated test for multi-period relay. | Low |
