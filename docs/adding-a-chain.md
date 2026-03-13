# Adding a New Chain

Use the Cosmos implementation (`crates/chains/cosmos/`) as reference throughout.

## 1. Create the crate

```bash
cargo init crates/chains/mychain --lib
```

Add to workspace `Cargo.toml` members, then add `mercury-chain-traits` and `mercury-core` as dependencies.

## 2. Module layout

Mirror the Cosmos crate structure:

- `chain.rs` — main struct + constructor, `HasChainTypes` and `HasIbcTypes<Self>` impls
- `config.rs` — TOML-deserializable config
- `keys.rs` — signing
- `types.rs` — events, packets, proofs, chain status
- `queries.rs` — `CanQueryChainStatus`, `CanQueryClient<Self>`, `CanQueryPacketState<Self>`
- `events.rs` — `CanExtractPacketEvents`, `CanQueryBlockEvents` (parse SendPacket/WriteAck from raw events)
- `builders.rs` — `CanBuildClientPayloads<Self>`, `CanBuildClientMessages<Self>`, `CanBuildPacketMessages<Self>`
- `messaging.rs` / `tx.rs` — `CanSendMessages`, `HasTxTypes`, transaction submission

## 3. Implement traits

All traits live in `mercury-chain-traits`. Implement them in order:

1. **Type traits** — `HasChainTypes` (height, timestamp, chain ID, events, messages, chain status, revision number), `HasIbcTypes<Self>` (client ID, client/consensus state, proofs, packets, acknowledgements)
2. **Query traits** — `CanQueryChainStatus`, `CanQueryClient<Self>` (client state, consensus state, trusting period, client latest height), `CanQueryPacketState<Self>` (packet commitment/receipt/ack with Merkle proofs), `CanQueryBlockEvents`
3. **Event extraction** — `CanExtractPacketEvents<Self>` (SendPacket, WriteAck)
4. **Builder traits** — `CanBuildClientPayloads<Self>` (create/update client payloads), `CanBuildClientMessages<Self>` (create/update client, register counterparty), `CanBuildPacketMessages<Self>` (recv/ack/timeout packets)
5. **Transaction traits** — `HasTxTypes`, `CanSubmitTx`, `CanEstimateFee`, `CanQueryNonce`, `CanPollTxResponse`
6. **Messaging** — `CanSendMessages` with batching and nonce retry

Once all traits are implemented, `Chain<Self>` is automatically satisfied via a blanket impl.

## 4. Wire into the CLI

In `crates/cli/`:

- `config.rs` — add variant to `ChainConfig` enum
- `main.rs` — add variant to `ConnectedChain` enum, handle in `connect_chain()` and `spawn_relay_pair()`

## 5. Cross-chain support

To relay between your chain and Cosmos (or another chain), implement `HasIbcTypes` and the builder/query traits with the counterparty as the generic parameter — on both sides.
