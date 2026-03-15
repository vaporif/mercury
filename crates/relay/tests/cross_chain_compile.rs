//! Compile-time verification that RelayContext can be instantiated
//! with cross-chain type parameters. If HasInner equality constraints
//! are missing, this will fail to compile.

mod cross_chain {
    use mercury_cosmos_bridges::CosmosChain;
    use mercury_cosmos_bridges::keys::Secp256k1KeyPair;
    use mercury_ethereum_bridges::EthereumChain;
    use mercury_relay::context::RelayContext;

    // These type aliases verify the bounds are satisfiable.
    // They don't need to be used at runtime.
    type _CosmosToEth = RelayContext<CosmosChain<Secp256k1KeyPair>, EthereumChain>;
    type _EthToCosmos = RelayContext<EthereumChain, CosmosChain<Secp256k1KeyPair>>;
}
