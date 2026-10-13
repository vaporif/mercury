# Solana→Cosmos via IBC attestors

## Problem

Cosmos tracks Solana with `cw_dummy_light_client` and the relayer sends `b"mock"` proofs, so nothing checks that a Solana packet was ever committed. Solana has no state proofs a light client could verify. Upstream (`cosmos/ibc-contracts`, formerly solidity-ibc-eureka) solves this with ibc-go's native `attestations` light client (ibc-go v11) fed by signatures from [`cosmos/ibc-attestor`](https://github.com/cosmos/ibc-attestor) instances that read Solana at `finalized` commitment.

Scope: Solana→Cosmos only. Mercury's Ethereum attested mode (HTTP/JSON) is left as is.

## Protocol facts this design relies on

| Fact | Source |
|---|---|
| Attestor speaks gRPC `ibc_attestor.AttestationService`: `LatestHeight`, `StateAttestation(height)`, `PacketAttestation(packets, height, commitment_type)` | `ibc-attestor/proto/ibc_attestor/*.proto` |
| Packets are sent ABI-encoded as Solidity `IICS26RouterMsgs.Packet` | `ibc-attestor/apps/ibc-attestor/src/lib.rs` (`Packet::abi_decode`) |
| Attestor rejects heights above its finalized slot | `rpc/attestor.rs::validate_height` |
| Solana adapter reads the commitment/ack/receipt PDA at `finalized` commitment (ignores the requested slot) | `adapter/solana.rs::get_commitment` |
| Each attestor response holds one 65-byte signature; quorum = N responses with identical `attested_data` | upstream `proof-api/lib/src/aggregator/attestor_data.rs` |
| State attestation timestamp is in seconds; ibc-go converts it to ns | `ibc-go/.../attestations/abi.go` |
| Client message for updates: `AttestationProof{attestation_data, signatures}` | `ibc-go/.../attestations/update.go` |
| Membership/non-membership proofs: protobuf `AttestationProof`; requires a consensus state at the proof height **and** packet attestation height == proof height | `ibc-go/.../attestations/client_state.go` |
| Updating to any height (also lower or equal) just stores the consensus state | `update.go::UpdateState` |

## Design

### New crate `crates/attestor` (`mercury-attestor`)

Chain-agnostic, so the Ethereum path can move onto it later.

- Hand-written prost messages for the attestor protos and `ibc.lightclients.attestations.v1` (`ClientState`, `ConsensusState`, `AttestationProof`). No `protoc`/build script, matching the rest of the workspace.
- `AttestorClient`: one tonic channel per endpoint, unary calls through `tonic::client::Grpc` + `ProstCodec`, per-call timeout.
- `Aggregator` over N clients with `quorum_threshold`:
  - `latest_height()`: query all, require quorum of responses, return the **minimum**, so every quorum member can attest it.
  - `state_attestation(h)` / `packet_attestation(packets, h, kind)`: query all, group by `attested_data`, return the first group with `>= quorum` signatures as `AggregatedAttestation { height, timestamp, attested_data, signatures }`. Responses with signatures that aren't 65 bytes are dropped.
- `abi_encode_packet(&ibc_proto v2 Packet) -> Vec<u8>` via `alloy-sol-types` `sol!` definitions matching `IICS26RouterMsgs.Packet`.

### Solana chain (`mercury-solana`)

- Config: optional `[attestation]` table on the Solana chain config:
  ```toml
  [attestation]
  endpoints = ["http://127.0.0.1:2025"]
  attestor_addresses = ["0xabc..."]   # Ethereum-style addresses of the attestor keys
  min_required_sigs = 1
  timeout_secs = 30                     # optional
  ```
  Validation: endpoints and addresses non-empty, `1 <= min_required_sigs <= addresses.len()`, endpoints are http(s) URLs.
- `SolanaChain` holds `Option<Arc<Aggregator>>`, built in `new` when the table is present (lazy channel connect so construction stays sync).
- `ClientPayloadBuilder` in attested mode:
  - create: `H = latest_height()`, `state_attestation(H)` for the timestamp; payload variant `Attested { attestor_addresses, min_required_sigs, height, timestamp_secs }`.
  - update: `H = latest_height()`, `state_attestation(H)`; payload variant `Attested { height: H, proof: AttestationProof bytes }`. `update_payload_proof_height` returns `H`.
- `SolanaCommitmentProof` becomes an enum: `None` (today's empty proof) or `Attested(Arc<Aggregator>)`. Packet queries return `Attested` when the chain is in attested mode. The proof is completed on the destination side, where the full packet is known (see below). This avoids changing `PacketStateQuery`, which only receives a sequence number while the attestor needs the whole packet.

### Cosmos counterparty for Solana

- `build_create_client_message`: `Attested` payload → `MsgCreateClient` with `/ibc.lightclients.attestations.v1.ClientState` and `ConsensusState{timestamp: secs * 1e9}`. The dummy wasm path is unchanged.
- `build_update_client_message`: `Attested` payload → `MsgUpdateClient` with `/ibc.lightclients.attestations.v1.AttestationProof`.
- recv / ack / timeout builders: when the proof is `Attested`, ABI-encode the v2 packet, call `packet_attestation([packet], proof_height, Packet|Ack|Receipt)`, and put the protobuf `AttestationProof` in `proof_commitment` / `proof_acked` / `proof_unreceived` with `proof_height = H`. `mock_proofs` keeps working as today.
- `CosmosClientState` / `CosmosConsensusState` gain an `Attestations` variant so `query_client_state` can decode the client; `client_latest_height` reads `latest_height`, trusting period is `None` (attestation clients don't expire). All existing matches get the new arm.

### Relay: timeout proof height

`build_src_update_client_messages` (used for timeouts) ignores `update_payload_proof_height` / `update_payload_message_height` today and uses the destination's latest height. For attested clients that height has no consensus state and isn't finalized. Change it to return the proof height (query) and message height, the same way `build_dst_update_client_payload` does, and pass the message height through `build_timeout_packet_messages` as an optional override (mirrors `build_receive_packet_messages`). This also fixes the same mismatch for beacon clients.

### Height and finality

The packet worker updates the client to `H = min finalized slot across attestors`, then asks for packet attestations at `H`. A packet whose commitment isn't finalized yet makes the attestor return `CommitmentNotFound`; the worker's normal retry picks it up on the next pass once Solana finalizes (~13s on localnet). No extra wait loop.

### e2e

- Cosmos image: the existing `ghcr.io/cosmos/ibc-go-wasm-simd:main` already registers the `attestations` client (as does upstream's `v11.0.0`), so it stays.
- Attestor: `ghcr.io/cosmos/ibc-attestor:v0.3.0` (upstream e2e pin; newer `main` requires a keystore password).
  1. Generate a key per attestor in a one-shot container (`key generate` + `key show`), reading the address and keystore JSON from stdout. No bind mounts, so container UIDs don't matter.
  2. Start `server --config ... --chain-type solana` with host networking, copying the keystore and config in. `solana-test-validator` binds to `127.0.0.1` by default, so the attestor uses `http://127.0.0.1:8899` and listens on a free local port.
  3. Wait for the `gRPC server ready` log line.
- `cosmos_solana` harness creates an `attestations` client on Cosmos instead of the dummy wasm client and sets `mock_proofs: false`. Bidirectional test asserts that the Solana→Cosmos packet is received and its ack is relayed back to Solana (the router zeroes the commitment value; the account stays).
- The Solana client's counterparty merkle prefix registered on Cosmos is `[""]`. Solana commitments have no store prefix, and the `attestations` client rejects key paths with more than one element.

### Found while testing

Relaying a Cosmos ack to Solana sent the proto-encoded `Acknowledgement` as `MsgAckPacket.acknowledgement`. The router hashes that field as a single app acknowledgement, so `ics07-tendermint` rejected the proof. The Solana builder now unwraps `app_acknowledgements[0]` (as upstream does) and rejects multi-payload acks.
- One attestor, quorum 1 (upstream default). A unit test covers quorum aggregation with several responses.

## Out of scope

- Moving Ethereum's attested mode to gRPC.
- Batching several packets into one attestation request (one request per packet for now; the attestor caps batches at 100).
- Misbehaviour handling for attestation clients.
