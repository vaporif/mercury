# Mercury E2E Tests

End-to-end tests that spin up real chain infrastructure (Docker containers, Anvil) and run the full relay pipeline.

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
| Near-expiry timeout | Packet arriving just before timeout — verify delivery, not timeout. | Low |
| Chain downtime | Destination unreachable during relay, then recovers. | Low |

## Cosmos <-> Ethereum

### Covered

| Test | What it validates |
|------|-------------------|
| `bootstrap_smoke` | Anvil bootstrap: contracts deployed, IBC handler ready |
| `cosmos_to_eth_transfer` | Cosmos->Ethereum unidirectional with ABI-encoded ICS20 packet |
| `cosmos_eth_roundtrip_transfer` | Cosmos->Eth->Cosmos roundtrip (exists but return leg blocked — see gaps) |

### Gaps

| Gap | Description | Priority |
|-----|-------------|----------|
| Eth->Cosmos direction blocked | `build_update_client_payload_mock()` returns empty headers; wasm client never advances past height {0,0}. Blocks all Eth->Cosmos tests. | **Critical** |
| Protobuf wrapping | `MsgUpdateClient.client_message` puts raw bytes in `Any.value` instead of `ClientMessage { data }` wrapper. ibc-go can't unmarshal. | **Critical** |
| Eth->Cosmos unidirectional | No standalone test — only attempted inside blocked roundtrip. | High |
| Bidirectional transfer | B->A un-escrowing not tested (blocked by above). | High |
| Packet timeout (Cosmos side) | No test for packets timing out when Cosmos is destination. | High |
| Packet timeout (Eth side) | No test for packets timing out on Ethereum. | Medium |
| Client refresh | No equivalent of `client_refresh_keeps_relay_alive` for Eth<->Cosmos. | Medium |
| Clearing worker recovery | Same gap as Cosmos<->Cosmos — never exercised. | Medium |
| Concurrent transfers | No parallel traffic test for Cosmos<->Eth. | Medium |
| Binary relay mode | No subprocess/CLI test for Eth relay (only library mode tested). | Low |
| Multiple denominations | Only `"stake"` tested. | Low |
| Real beacon client | All Eth tests use mock/dummy light clients. No E2E with actual beacon chain. | Low (infra-heavy) |
