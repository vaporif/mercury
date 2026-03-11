# IBC v2 (Eureka)

Mercury targets IBC v2 instead of v1.

## What Changed

**IBC v1** has clients, connections (4-step handshake), channels (4-step handshake), and packets. Setting up a relay path requires 8+ transactions across two chains. The relayer must manage connection and channel lifecycle state machines.

**IBC v2** has clients, counterparty registration (1 message, no handshake), and packets. Setup requires 4 transactions. Connections and channels are gone.

## Impact on the Relayer

The connection and channel handshake logic accounted for ~16 traits in hermes-sdk. Four-step handshakes require tracking intermediate states, retrying on timeouts, and coordinating messages across both chains in a specific order. All of that is eliminated.

What remains:

- **Client lifecycle** — create and update light clients on both chains
- **Counterparty registration** — one message per chain, no handshake coordination
- **Packet relay** — the core loop: watch for `SendPacket`, build `RecvPacket` with proofs, relay acknowledgements

Combined with dropping CGP, Mercury goes from hermes-sdk's 250+ components to ~35 traits.

## Packet Flow (v2)

```
Source Chain                          Destination Chain
     |                                      |
     |  SendPacket(payload, ...)            |
     |------------------------------------->|
     |                                      |
     |         RecvPacket(packet, proof)     |
     |<-------------------------------------|
     |                                      |
     |  Acknowledgement(ack, proof)         |
     |------------------------------------->|
     |                                      |
```

If the destination doesn't receive the packet before the timeout, the source chain processes a timeout message instead of an acknowledgement. The relayer handles both paths.

## Why v2 Now

IBC v2 is actively being deployed. The [solidity-ibc-eureka](https://github.com/cosmos/solidity-ibc-eureka) contracts implement it for EVM chains, and Cosmos SDK chains are adding support. Building a new relayer on v1 would mean targeting a protocol version that's being superseded.

Starting fresh on v2 means Mercury doesn't carry any v1 legacy — no connection or channel types, no handshake state machines, no compatibility shims.
