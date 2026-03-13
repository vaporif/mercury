# Adding a New Chain

Use the Cosmos implementation (`crates/chains/cosmos/`) as reference throughout.

## 1. Create the crate

```bash
cargo init crates/chains/mychain --lib
```

Add to workspace `Cargo.toml` members, then add `mercury-chain-traits` and `mercury-core` as dependencies.

## 2. Module layout

Mirror the Cosmos crate structure:

- `chain.rs` — main struct + constructor, `ChainTypes` and `IbcTypes<Self>` impls
- `config.rs` — TOML-deserializable config
- `keys.rs` — signing
- `types.rs` — events, packets, proofs, chain status
- `queries.rs` — `ChainStatusQuery`, `ClientQuery<Self>`, `PacketStateQuery<Self>`
- `events.rs` — `PacketEvents<Self>` (parse SendPacket/WriteAck from raw events, query block events)
- `builders.rs` — `ClientPayloadBuilder<Self>`, `ClientMessageBuilder<Self>`, `PacketMessageBuilder<Self>`
- `tx.rs` — `MessageSender`, transaction building, signing, fee estimation, submission

## 3. Implement traits

All traits live in `mercury-chain-traits`. Implement them in order:

1. **Type traits** — `ChainTypes` (height, timestamp, chain ID, events, messages, chain status, revision number, increment height), `IbcTypes<Self>` (client ID, client/consensus state, proofs, packets, acknowledgements)
2. **Query traits** — `ChainStatusQuery`, `ClientQuery<Self>` (client state, consensus state, trusting period, client latest height), `PacketStateQuery<Self>` (packet commitment/receipt/ack with Merkle proofs)
3. **Events** — `PacketEvents<Self>` (extract SendPacket/WriteAck from raw events, query block events)
4. **Builder traits** — `ClientPayloadBuilder<Self>` (create/update client payloads), `ClientMessageBuilder<Self>` (create/update client, register counterparty), `PacketMessageBuilder<Self>` (recv/ack/timeout packets)
5. **Messaging** — `MessageSender` with batching and nonce retry
6. **Transaction internals** — fee estimation, nonce queries, tx submission, polling (concrete methods, not traits)

Once all traits are implemented, `Chain<Self>` is automatically satisfied via a blanket impl.

## 4. Wire into the CLI

In `crates/cli/`:

- `config.rs` — add variant to `ChainConfig` enum
- `main.rs` — add variant to `ConnectedChain` enum, handle in `connect_chain()` and `spawn_relay_pair()`

## 5. Cross-chain support

To relay between your chain and Cosmos (or another chain), implement `IbcTypes` and the builder/query traits with the counterparty as the generic parameter — on both sides.
