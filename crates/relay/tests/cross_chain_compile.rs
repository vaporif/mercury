//! Compile-time verification that `RelayContext` can be instantiated
//! with cross-chain type parameters. If `HasInner` equality constraints
//! are missing, this will fail to compile.
//!
//! Note: type aliases don't fully check trait bounds — they verify
//! structural compatibility. Use `fn _assert<T: Trait>() {}` for
//! exhaustive bound checking once all cross-chain impls are complete.

mod cross_chain {
    use mercury_cosmos_bridges::CosmosChain;
    use mercury_cosmos_bridges::keys::Secp256k1KeyPair;
    use mercury_ethereum_bridges::EthereumChain;
    use mercury_relay::context::RelayContext;

    type _CosmosToEth = RelayContext<CosmosChain<Secp256k1KeyPair>, EthereumChain>;
    type _EthToCosmos = RelayContext<EthereumChain, CosmosChain<Secp256k1KeyPair>>;
}
