# Adding a New Chain

Use the Cosmos implementation (`crates/chains/cosmos/`) as reference throughout.

## 1. Create the crate

```bash
cargo init crates/chains/mychain --lib
```

Add to workspace `Cargo.toml` members, then add `mercury-chain-traits` and `mercury-core` as dependencies.

## 2. Module layout

Mirror the Cosmos crate structure:

- `chain.rs` — main struct + constructor, `ChainTypes` and `IbcTypes` impls
- `config.rs` — TOML-deserializable config
- `keys.rs` — signing
- `types.rs` — events, packets, proofs, chain status
- `queries.rs` — `ChainStatusQuery`, `ClientQuery<Self>`, `PacketStateQuery`
- `events.rs` — `PacketEvents` (parse SendPacket/WriteAck from raw events, query block events)
- `builders.rs` — `ClientPayloadBuilder<Self>`, `ClientMessageBuilder<Self>`, `PacketMessageBuilder<Self>`
- `tx.rs` — `MessageSender`, transaction building, signing, fee estimation, submission

## 3. Implement traits

All traits live in `mercury-chain-traits`. Implement them in order:

1. **Type traits** — `ChainTypes` (height, timestamp, chain ID, events, messages, chain status, revision number, increment height), `IbcTypes` (client/consensus state, proofs, packets, acknowledgements)
2. **Query traits** — `ChainStatusQuery`, `ClientQuery<Self>` (client state, consensus state, trusting period, client latest height), `PacketStateQuery` (packet commitment/receipt/ack with Merkle proofs)
3. **Events** — `PacketEvents` (extract SendPacket/WriteAck from raw events, query block events)
4. **Builder traits** — `ClientPayloadBuilder<Self>` (create/update client payloads), `ClientMessageBuilder<Self>` (create/update client, register counterparty), `PacketMessageBuilder<Self>` (recv/ack/timeout packets)
5. **Messaging** — `MessageSender` with batching and nonce retry
6. **Transaction internals** — fee estimation, nonce queries, tx submission, polling (concrete methods, not traits)

Once all traits are implemented, `RelayChain` is automatically satisfied via a blanket impl.

## 4. Implement the plugin

In your counterparty crate (e.g., `mercury-mychain-counterparties/src/plugin.rs`):

1. **`ChainPlugin`** — implement `chain_type()`, `validate_config()`, `connect()`, `parse_client_id()`, `query_status()`, `chain_id_from_config()`, `rpc_addr_from_config()`. The `connect()` method creates your chain, wraps it in `CachedChain`, and returns it as `AnyChain` (`Arc<dyn Any + Send + Sync>`).

2. **`RelayPairPlugin`** — for each supported relay direction, implement `src_type()`, `dst_type()`, and `build_relay()`. The `build_relay()` method downcasts `AnyChain` back to your concrete types, creates a `RelayContext`, and returns forward + reverse `DynRelay` instances.

3. **`register()` function** — register your chain plugin and all relay pair plugins with the `ChainRegistry`:

```rust
pub fn register(registry: &mut ChainRegistry) {
    registry.register_chain(MyChainPlugin);
    registry.register_pair(MyChainToMyChainRelay);
    // Add cross-chain relay pairs as needed
}
```

4. **Wire into the CLI** — add your `register()` call in `crates/cli/src/registry.rs`:

```rust
pub fn build_registry() -> ChainRegistry {
    let mut r = ChainRegistry::new();
    mercury_cosmos_counterparties::plugin::register(&mut r);
    mercury_ethereum_counterparties::plugin::register(&mut r);
    mercury_mychain_counterparties::plugin::register(&mut r);  // add this
    r
}
```

No enum variants, match arms, or CLI code changes needed beyond this single line.

## 5. Cross-chain support

To relay between your chain and an existing chain, you need cross-chain trait impls on **both sides**. Each side lives in its respective counterparty crate, behind a feature flag.

### What to implement

For a new chain `MyChain` relaying against Cosmos:

**In `mercury-mychain-counterparties/`** (your counterparty crate):
- `ClientPayloadBuilder<CosmosChain<S>>` — builds your chain's light client payloads. `build_create_client_payload` is typically counterparty-agnostic. `build_update_client_payload` receives `CosmosClientState`, which is an enum — match on the variant that wraps your light client (usually `Wasm` for non-Tendermint clients).
- `ClientMessageBuilder<CosmosChain<S>>` — builds on-chain messages from Cosmos payloads
- `PacketMessageBuilder<CosmosChain<S>>` — builds recv/ack/timeout messages
- `ClientQuery<CosmosChain<S>>` — queries your chain for Cosmos client/consensus state
- `MisbehaviourDetector<CosmosChain<S>>` + `MisbehaviourQuery` + `MisbehaviourMessageBuilder` — can be no-op stubs initially

**In `mercury-cosmos-counterparties/`** (the Cosmos counterparty crate):
- `ClientPayloadBuilder<MyChain>` — Cosmos's impl is fully generic (`impl<C: ChainTypes> ClientPayloadBuilder<C>`), so this is automatic via the blanket forward
- `ClientMessageBuilder<MyChain>` — builds `MsgCreateClient`/`MsgUpdateClient` on Cosmos targeting your chain's light client
- `PacketMessageBuilder<MyChain>` — builds Cosmos packet messages from your chain's proof types
- `ClientQuery<MyChain>` — queries Cosmos for your chain's client state (dispatches on `CosmosClientState` enum)

### Adapter forwarding pattern

Each chain has an adapter type (`MyChainAdapter`) in its counterparty crate that forwards same-chain traits from the core type and adds cross-chain impls. The `delegate_chain!` macro handles all boilerplate delegation:

```rust
// Generates Deref, HasCore, ChainTypes, IbcTypes, and all operational trait delegations.
// Also generates a blanket ClientPayloadBuilder<C> delegation by default.
mercury_chain_traits::delegate_chain! {
    impl[S: MySigner] MyChainAdapter<S> => MyChain<S>
}

// Use skip_cpb if your adapter needs a custom ClientPayloadBuilder impl:
mercury_chain_traits::delegate_chain! {
    impl[] MyChainAdapter => MyChain; skip_cpb
}
```

Cross-chain trait impls (`ClientMessageBuilder<OtherChain>`, `PacketMessageBuilder<OtherChain>`) are still written manually on the adapter, since they contain counterparty-specific logic (e.g., Ethereum must unwrap `CosmosClientState::Wasm` to extract beacon bytes).

### CosmosClientState enum

Non-native light clients on Cosmos are deployed as CosmWasm contracts, so their state is wrapped in `CosmosClientState::Wasm`. The `data` field contains the inner light client state bytes (e.g., JSON-serialized beacon client state for Ethereum). When implementing cross-chain traits against Cosmos, match on this enum to extract your chain's inner state. The exhaustive match ensures a compile error if a new variant is added, forcing explicit handling.

### CLI wiring

Add your counterparty crate's `register()` call to `crates/cli/src/registry.rs`. The plugin system handles chain connection, relay construction, and status queries automatically — no enum variants or match arms needed in the CLI.
