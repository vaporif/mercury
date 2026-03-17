// Compile time checks
mod cross_chain {
    use mercury_cosmos_counterparties::CosmosAdapter;
    use mercury_cosmos_counterparties::keys::Secp256k1KeyPair;
    use mercury_ethereum_counterparties::EthereumAdapter;
    use mercury_relay::context::RelayContext;

    type _CosmosToEth = RelayContext<CosmosAdapter<Secp256k1KeyPair>, EthereumAdapter>;
    type _EthToCosmos = RelayContext<EthereumAdapter, CosmosAdapter<Secp256k1KeyPair>>;
}
