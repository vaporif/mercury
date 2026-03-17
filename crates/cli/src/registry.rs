use mercury_core::registry::ChainRegistry;

pub fn build_registry() -> ChainRegistry {
    let mut r = ChainRegistry::new();
    mercury_cosmos_counterparties::plugin::register(&mut r);
    mercury_ethereum_counterparties::plugin::register(&mut r);
    mercury_cosmos_ethereum_relay::plugin::register(&mut r);
    r
}
