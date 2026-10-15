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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_all_shipped_chain_types() {
        let registry = build_registry();
        for chain_type in ["cosmos", "ethereum", "solana"] {
            assert_eq!(registry.chain(chain_type).unwrap().chain_type(), chain_type);
        }
    }

    #[test]
    fn registers_both_directions_of_every_relay_pair() {
        let registry = build_registry();
        for (a, b) in [
            ("cosmos", "cosmos"),
            ("cosmos", "ethereum"),
            ("cosmos", "solana"),
        ] {
            registry.pair(a, b).unwrap();
            registry.pair(b, a).unwrap();
        }
        assert!(registry.pair("ethereum", "solana").is_err());
    }
}
