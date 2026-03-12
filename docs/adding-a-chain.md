# Adding a New Chain

Use the Cosmos implementation (`crates/chains/cosmos/`) as reference throughout.

## 1. Create the crate

```bash
cargo init crates/chains/mychain --lib
```

Add to workspace `Cargo.toml` members, then add `mercury-chain-traits` and `mercury-core` as dependencies.

## 2. Module layout

Mirror the Cosmos crate structure:

- `chain.rs` — main struct + constructor
- `config.rs` — TOML-deserializable config
- `keys.rs` — signing
- `types.rs` — events, packets, proofs
- `status.rs` / `queries.rs` / `packet_queries.rs` — chain queries
- `events.rs` — parse SendPacket/WriteAck from raw events
- `message_builders.rs` / `packet_builders.rs` / `payload_builders.rs` — IBC message construction
- `messaging.rs` / `tx.rs` — transaction submission

## 3. Implement traits

All traits live in `mercury-chain-traits`. Implement them in order:

1. **Type traits** — `HasChainTypes`, `HasMessageTypes`, `HasIbcTypes`, `HasPacketTypes`, `HasChainStatusType`, `HasRevisionNumber`
2. **Query traits** — `CanQueryChainStatus`, `CanQueryClientState`, `CanQueryConsensusState`, `CanQueryBlockEvents`, packet commitment/receipt/ack queries (must return Merkle proofs)
3. **Event extraction** — `CanExtractPacketEvents` (SendPacket, WriteAck)
4. **Message/payload builders** — create/update client, register counterparty, recv/ack/timeout packets
5. **Transaction traits** — `HasTxTypes`, `CanSubmitTx`, `CanEstimateFee`, `CanQueryNonce`, `CanPollTxResponse`
6. **Messaging** — `CanSendMessages` with batching and nonce retry

## 4. Wire into the CLI

In `crates/cli/`:

- `config.rs` — add variant to `ChainConfig` enum
- `main.rs` — add variant to `ConnectedChain` enum, handle in `connect_chain()` and `spawn_relay_pair()`

## 5. Cross-chain support

To relay between your chain and Cosmos (or another chain), implement `HasIbcTypes`, `HasPacketTypes`, and the builder traits with the counterparty as the generic parameter — on both sides.
