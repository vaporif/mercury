# Misbehaviour Detection — Design Spec

## Overview

Detect and submit evidence when a light client is fed conflicting headers. Security-critical: without this, a compromised validator set can forge state proofs undetected.

## Key Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Detection depth | Full light client divergence via `tendermint-light-client-detector` | Catches both fork detection and BFT time violations |
| Architecture | Dedicated `MisbehaviourWorker` | Clean separation from relay hot path, matches existing worker pattern |
| On detection | Abort the relay pair (both directions) | Compromised light client means the entire connection is untrustworthy |
| Scanning | Startup scan + periodic monitoring | Catches misbehaviour that occurred while relayer was offline |
| Trait design | Split: `MisbehaviourDetector` (src) + `MisbehaviourMessageBuilder` (dst) | Matches existing PayloadBuilder/MessageBuilder split; src has RPC for detection, dst has signer for message |
| Submission orchestration | Worker handles supporting headers, message builder returns single message | Worker orchestrates submission order; supporting headers via existing `build_update_client_message` |
| Misbehaviour message | `MsgUpdateClient` with misbehaviour as `client_message` | `MsgSubmitMisbehaviour` is deprecated in ibc-go v7+; Mercury targets IBC v2 |

## Components

### 1. `MisbehaviourDetector` Trait (Source Chain)

**Location:** `crates/chain-traits/src/builders.rs`

Implemented by the **source chain** (where headers originate). Handles detection and evidence construction. Not added to the `Chain` composite trait — used as an additional bound on `MisbehaviourWorker`.

```rust
#[async_trait]
pub trait MisbehaviourDetector<Counterparty: ChainTypes + ?Sized>: IbcTypes<Counterparty> {
    type UpdateHeader: ThreadSafe;
    type MisbehaviourEvidence: ThreadSafe;

    /// Check a decoded update header against the source chain for divergence.
    /// Returns evidence if divergence detected, None if valid.
    async fn check_for_misbehaviour(
        &self,
        update_header: &Self::UpdateHeader,
        client_state: &<Counterparty as IbcTypes<Self>>::ClientState,
    ) -> Result<Option<Self::MisbehaviourEvidence>>;
}
```

**`UpdateHeader` associated type:** For Cosmos, this is `TmIbcHeader`. The query method (section 3) returns this type, and the worker passes it directly. No raw `Message` or `Event` decoding in the worker.

**`client_state` parameter:** `<Counterparty as IbcTypes<Self>>::ClientState` — the dst chain's representation of the src chain's client state. For Cosmos-to-Cosmos this resolves to `TendermintClientState`. Used to extract verifier options (trust threshold, trusting period, clock drift).

### 2. `MisbehaviourMessageBuilder` Trait (Destination Chain)

**Location:** `crates/chain-traits/src/builders.rs`

Implemented by the **destination chain** (where the message gets submitted). Wraps evidence into a submittable message using the dst chain's signer. Matches the existing `ClientMessageBuilder` pattern.

```rust
#[async_trait]
pub trait MisbehaviourMessageBuilder<Counterparty>: IbcTypes<Counterparty>
where
    Counterparty: ChainTypes + MisbehaviourDetector<Self>,
{
    /// Build a MsgUpdateClient containing the misbehaviour evidence.
    async fn build_misbehaviour_message(
        &self,
        client_id: &Self::ClientId,
        evidence: Counterparty::MisbehaviourEvidence,
    ) -> Result<Self::Message>;
}
```

The dst chain has its signer internally (same as `build_update_client_message`). The worker calls this after detection, same pattern as normal client updates.

### 3. Cosmos Implementation

**Location:** `crates/chains/cosmos/src/misbehaviour.rs` (new file)

**Evidence type:**

```rust
pub struct CosmosMisbehaviourEvidence {
    pub misbehaviour: TmMisbehaviour,
    pub supporting_headers: Vec<TmIbcHeader>,
}
```

**`MisbehaviourDetector` impl (src chain):**

- `UpdateHeader` = `TmIbcHeader`
- `MisbehaviourEvidence` = `CosmosMisbehaviourEvidence`

`check_for_misbehaviour`:

1. Accept a decoded `TmIbcHeader` (the header that was submitted on-chain)
2. Extract `trusted_height` from the header
3. Build a `LightBlock` from the header:
   - `signed_header` from the `TmIbcHeader`
   - `validator_set` from the `TmIbcHeader`
   - Fetch `next_validators` from RPC at `header.height + 1`
   - Set `provider` to the chain's peer ID
4. Fetch the trusted `LightBlock` from RPC at `trusted_height`:
   - Fetch `commit` + `validators` + `next_validators` via RPC (same pattern as `build_update_client_payload`)
5. Instantiate a Tendermint light client with:
   - The chain's RPC client as provider
   - Verifier options from the client state (trust threshold, trusting period, clock drift)
6. Call `detect_divergence` from the `tendermint-light-client-detector` crate
   - This requires `Provider` + `Verifier` instances; the Cosmos impl sets these up using the chain's RPC client
7. If `Some(Divergence)`: header1 = submitted header, header2 = challenging block, extract witness trace as supporting headers → return `CosmosMisbehaviourEvidence`
8. If `None` → return `Ok(None)`

**`MisbehaviourMessageBuilder` impl (dst chain):**

`build_misbehaviour_message`:

1. Proto-encode `evidence.misbehaviour` (`TmMisbehaviour` → `Any`)
2. Build `MsgUpdateClient` with:
   - `client_id`: the dst chain's client ID
   - `client_message`: the encoded `TmMisbehaviour`
   - `signer`: `self.signer.account_address()` (dst chain's relayer account)
3. Return as `CosmosMessage`

This mirrors `build_update_client_message` — same struct, different `client_message` content.

**New dependency:** `tendermint-light-client-detector` in `crates/chains/cosmos/Cargo.toml`. Must be compatible with existing `tendermint 0.40` / `tendermint-light-client-verifier 0.40` versions.

### 4. New Query Trait and Methods

**Location:** `crates/chain-traits/src/queries.rs`

New `MisbehaviourQuery` trait — separate from `ClientQuery` to avoid breaking existing impls. Used as an additional bound on `MisbehaviourWorker`.

```rust
#[async_trait]
pub trait MisbehaviourQuery<Counterparty: ChainTypes + ?Sized>: IbcTypes<Counterparty>
where
    Counterparty: MisbehaviourDetector<Self>,
{
    /// List all consensus state heights for a client, in descending order.
    async fn query_consensus_state_heights(
        &self,
        client_id: &Self::ClientId,
    ) -> Result<Vec<Counterparty::Height>>;

    /// Returns the decoded header from the UpdateClient tx at the given consensus height.
    /// Returns None if the event has been pruned from the tx index.
    async fn query_update_client_header(
        &self,
        client_id: &Self::ClientId,
        consensus_height: &Counterparty::Height,
    ) -> Result<Option<Counterparty::UpdateHeader>>;
}
```

**Key design:** `query_update_client_header` returns `Counterparty::UpdateHeader` (the `MisbehaviourDetector`'s associated type). For Cosmos this is `TmIbcHeader`. The query decodes the raw event internally — the worker gets a typed header it can pass directly to `check_for_misbehaviour`.

**Cosmos implementation:**

- `query_consensus_state_heights`: gRPC `/ibc.core.client.v1.Query/ConsensusStateHeights`, returns heights in descending order
- `query_update_client_header`: `tx_search` for `update_client.client_id` + `update_client.consensus_heights` attributes, decode `header` attribute from hex into `TmIbcHeader`

**Known limitation:** If the node's tx index has been pruned, `query_update_client_header` returns `None`. The startup scan will skip those heights with a `warn!` log.

### 5. `MisbehaviourWorker`

**Location:** `crates/relay/src/workers/misbehaviour_worker.rs` (new file)

```rust
pub struct MisbehaviourWorker<R: Relay> {
    pub relay: Arc<R>,
    pub token: CancellationToken,
    pub scan_interval: Duration,
}
```

**Additional trait bounds on `R`:** `R::SrcChain: MisbehaviourDetector<R::DstChain>`, `R::DstChain: MisbehaviourQuery<R::SrcChain> + MisbehaviourMessageBuilder<R::SrcChain>`.

**Spawning:** One worker per relay direction inside `run_with_token`. Direction A→B monitors A's headers on B's light client. Direction B→A monitors B's headers on A's light client. Both are needed — they check different light clients.

**Worker flow (per scan):**

```
dst_chain.query_consensus_state_heights(dst_client_id)
    → for each new height:
        dst_chain.query_update_client_header(dst_client_id, height)
            → if Some(header):
                src_chain.check_for_misbehaviour(header, client_state)
                    → if Some(evidence):
                        dst_chain.build_update_client_message(supporting_headers)  // separate tx
                        dst_chain.build_misbehaviour_message(evidence)             // misbehaviour tx
                        dst_chain.send_messages(...)
                        token.cancel()
```

**Lifecycle:**

1. **Frozen client check** — Query dst chain's client state via existing `query_client_state`. Check `TendermintClientState.frozen_height` — if set (non-zero height), log info and skip scanning. Re-check each interval. (Cosmos impl accesses this directly; no new trait method needed for the initial Cosmos-only implementation.)
2. **Startup scan** — Query all consensus state heights from dst chain. For each, fetch the update header, run detection. Track the highest scanned height.
3. **Periodic monitoring** — After initial scan, poll on `scan_interval`. Check only new heights above the last scanned height.
4. **On evidence found:**
   - Build `MsgUpdateClient` for each supporting header via existing `build_update_client_message` — submit in a **separate transaction** first (avoids exceeding tx size limits)
   - Build misbehaviour message via `dst_chain.build_misbehaviour_message(evidence)`
   - Submit via `dst_chain.send_messages()`
   - Log at `error!` level with client_id, heights, chain details
   - `token.cancel()` → shuts down both directions of the relay pair
5. **On scan error** — Log `warn!`, continue to next interval.
6. **On pruned event** — Log `warn!` with consensus height, skip to next height.

### 6. Relay Pair Shutdown Plumbing

**Location:** `crates/cli/src/main.rs` (`spawn_relay_pair`) and `crates/relay/src/context.rs`

Currently `spawn_relay_pair` calls `fwd.run(worker_config)` and `rev.run(worker_config)`, each of which internally creates its own `CancellationToken` via `run()` → `run_with_token(CancellationToken::new())`.

**Change:** `spawn_relay_pair` creates a shared `CancellationToken` and calls `run_with_token(shared_token.clone())` on both directions. The `MisbehaviourWorker` receives this same token. On evidence submission, cancelling the shared token shuts down both directions. Other relay pairs are unaffected.

### 7. Configuration

**Location:** `crates/relay/src/context.rs` and `crates/cli/src/config.rs`

Add to `RelayWorkerConfig`:

```rust
pub struct RelayWorkerConfig {
    pub lookback: Option<Duration>,
    pub clearing_interval: Option<Duration>,
    pub misbehaviour_scan_interval: Option<Duration>,  // new; None = disabled
}
```

**Config file:** Add `misbehaviour_scan_interval_secs: Option<u64>` to `RelayConfig` in `config.rs`, following the same pattern as `clearing_interval_secs`.

When `Some(interval)`, spawn `MisbehaviourWorker` in `run_with_token` alongside other workers. Suggested default when enabled: 30 seconds.

## Error Handling

- Scan errors (RPC failures, decode errors): warn and continue to next interval
- Individual consensus height failures: skip and continue, don't abort full scan
- Pruned tx index events: warn with height, skip — known limitation
- Evidence submission failure: log error, retry on next scan interval (evidence remains valid)
- Frozen/expired client: check `TendermintClientState.frozen_height` directly in Cosmos impl, skip scan and log info if frozen

## Files Changed

| File | Change |
|---|---|
| `crates/chain-traits/src/builders.rs` | Add `MisbehaviourDetector` and `MisbehaviourMessageBuilder` traits |
| `crates/chain-traits/src/queries.rs` | Add `MisbehaviourQuery` trait |
| `crates/chains/cosmos/src/misbehaviour.rs` | New: Cosmos impls for `MisbehaviourDetector`, `MisbehaviourMessageBuilder`, `MisbehaviourQuery` + evidence type |
| `crates/chains/cosmos/src/queries.rs` | Implement `MisbehaviourQuery` |
| `crates/chains/cosmos/Cargo.toml` | Add `tendermint-light-client-detector` dependency |
| `crates/relay/src/workers/misbehaviour_worker.rs` | New: `MisbehaviourWorker` |
| `crates/relay/src/context.rs` | Add `misbehaviour_scan_interval` to config, spawn worker |
| `crates/cli/src/main.rs` | Shared `CancellationToken` in `spawn_relay_pair` |
| `crates/cli/src/config.rs` | Add `misbehaviour_scan_interval_secs` config field |

## Known Limitations

- **Pruned tx index:** If the node has pruned `UpdateClient` transaction events, the startup scan cannot retroactively detect misbehaviour for those heights.
- **No metrics:** Misbehaviour detection/submission counters will be added when Prometheus metrics (roadmap item #2) lands.
- **Single chain type:** Only Cosmos/Tendermint is supported. The traits are generic for future chain types but only Cosmos implements them initially.
