use mercury_core::registry::ChainRegistry;

pub fn build_registry() -> ChainRegistry {
    let mut r = ChainRegistry::new();
    mercury_cosmos_counterparties::plugin::register(&mut r);
    mercury_ethereum_counterparties::plugin::register(&mut r);
    mercury_solana_counterparties::plugin::register(&mut r);
    mercury_cosmos_cosmos_relay::register(&mut r);
    mercury_cosmos_ethereum_relay::register(&mut r);
    mercury_cosmos_solana_relay::register(&mut r);
    r
}
