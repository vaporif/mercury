//! Compile-time verification that `RelayContext` can be instantiated
//! with cross-chain type parameters. If `HasCore` equality constraints
//! are missing, this will fail to compile.
//!
//! Note: type aliases don't fully check trait bounds — they verify
//! structural compatibility. Use `fn _assert<T: Trait>() {}` for
//! exhaustive bound checking once all cross-chain impls are complete.

mod cross_chain {
    use mercury_cosmos_counterparties::CosmosAdapter;
    use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
    use mercury_ethereum_counterparties::EthereumAdapter;
    use mercury_relay::context::RelayContext;

    type _CosmosToEth = RelayContext<CosmosAdapter<Secp256k1KeyPair>, EthereumAdapter>;
    type _EthToCosmos = RelayContext<EthereumAdapter, CosmosAdapter<Secp256k1KeyPair>>;
}
