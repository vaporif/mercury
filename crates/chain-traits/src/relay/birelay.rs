use mercury_core::ThreadSafe;

use super::context::Relay;

/// A bidirectional relay that holds both A-to-B and B-to-A relay contexts.
pub trait BiRelay: ThreadSafe {
    type RelayAToB: Relay;
    type RelayBToA: Relay<
            SrcChain = <Self::RelayAToB as Relay>::DstChain,
            DstChain = <Self::RelayAToB as Relay>::SrcChain,
        >;

    fn relay_a_to_b(&self) -> &Self::RelayAToB;
    fn relay_b_to_a(&self) -> &Self::RelayBToA;
}
