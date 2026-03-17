use std::collections::HashMap;

use crate::plugin::{ChainPlugin, RelayPairPlugin};

pub struct ChainRegistry {
    chains: HashMap<&'static str, Box<dyn ChainPlugin>>,
    pairs: HashMap<(&'static str, &'static str), Box<dyn RelayPairPlugin>>,
}

impl ChainRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            chains: HashMap::new(),
            pairs: HashMap::new(),
        }
    }

    pub fn register_chain(&mut self, plugin: impl ChainPlugin + 'static) {
        self.chains.insert(plugin.chain_type(), Box::new(plugin));
    }

    pub fn register_pair(&mut self, plugin: impl RelayPairPlugin + 'static) {
        self.pairs
            .insert((plugin.src_type(), plugin.dst_type()), Box::new(plugin));
    }

    pub fn chain(&self, chain_type: &str) -> eyre::Result<&dyn ChainPlugin> {
        self.chains
            .get(chain_type)
            .map(AsRef::as_ref)
            .ok_or_else(move || eyre::eyre!("unsupported chain type '{chain_type}'"))
    }

    pub fn pair(&self, src_type: &str, dst_type: &str) -> eyre::Result<&dyn RelayPairPlugin> {
        self.pairs
            .iter()
            .find(|((s, d), _)| *s == src_type && *d == dst_type)
            .map(|(_, v)| v.as_ref())
            .ok_or_else(|| eyre::eyre!("unsupported relay pair: {src_type} -> {dst_type}"))
    }
}

impl Default for ChainRegistry {
    fn default() -> Self {
        Self::new()
    }
}
