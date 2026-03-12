use mercury_chain_traits::relay::birelay::BiRelay;
use mercury_chain_traits::relay::Relay;

/// Holds two opposing relay directions for bidirectional relaying.
pub struct BiRelayContext<R1: Relay, R2: Relay> {
    pub relay_a_to_b: R1,
    pub relay_b_to_a: R2,
}

impl<R1, R2> BiRelay for BiRelayContext<R1, R2>
where
    R1: Relay,
    R2: Relay<SrcChain = R1::DstChain, DstChain = R1::SrcChain>,
{
    type RelayAToB = R1;
    type RelayBToA = R2;

    fn relay_a_to_b(&self) -> &R1 {
        &self.relay_a_to_b
    }

    fn relay_b_to_a(&self) -> &R2 {
        &self.relay_b_to_a
    }
}
