# IBC v2 (Eureka)

Mercury targets IBC v2 instead of v1.

## What changed

IBC v1 has clients, connections (4-step handshake), channels (4-step handshake), and packets. Setting up a relay path requires 8+ transactions across two chains. The relayer must manage connection and channel lifecycle state machines.

IBC v2 has clients, counterparty registration (1 message, no handshake), and packets. Setup requires 4 transactions. Connections and channels are gone.

## Impact on the relayer

The connection and channel handshake logic alone was ~16 traits in hermes-sdk. Tracking intermediate states, retrying on timeouts, coordinating messages across both chains in a specific order. Gone.

What's left:

- Client lifecycle - create and update light clients on both chains
- Counterparty registration - one message per chain, no handshake coordination
- Packet relay - the core loop: watch for `SendPacket`, build `RecvPacket` with proofs, relay acknowledgements

Combined with dropping CGP, Mercury goes from hermes-sdk's 250+ components to ~35 traits.

## Packet flow (v2)

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

## Why v2 now

IBC v2 is actively being deployed. The [solidity-ibc-eureka](https://github.com/cosmos/solidity-ibc-eureka) contracts implement it for EVM chains, and Cosmos SDK chains are adding support. Writing a new relayer against v1 at this point would be targeting a protocol that's being replaced.

Mercury doesn't carry any v1 baggage. No connection types, no channel types, no handshake state machines.
