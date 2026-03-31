use std::collections::HashMap;

use crate::plugin::{ChainPair, ChainPlugin, ClientBuilder, ClientMode, RelayPairPlugin};

#[derive(Default)]
pub struct ChainRegistry {
    chains: HashMap<&'static str, Box<dyn ChainPlugin>>,
    pairs: HashMap<ChainPair, Box<dyn RelayPairPlugin>>,
    client_builders: HashMap<ChainPair, Box<dyn ClientBuilder>>,
}

impl ChainRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_chain(&mut self, plugin: impl ChainPlugin + 'static) {
        self.chains.insert(plugin.chain_type(), Box::new(plugin));
    }

    pub fn register_pair(&mut self, key: ChainPair, plugin: Box<dyn RelayPairPlugin>) {
        self.pairs.insert(key, plugin);
    }

    pub fn register_client_builder(&mut self, key: ChainPair, builder: Box<dyn ClientBuilder>) {
        self.client_builders.insert(key, builder);
    }

    pub fn chain(&self, chain_type: &str) -> eyre::Result<&dyn ChainPlugin> {
        self.chains
            .get(chain_type)
            .map(AsRef::as_ref)
            .ok_or_else(move || eyre::eyre!("unsupported chain type '{chain_type}'"))
    }

    pub fn pair(
        &self,
        src_type: &str,
        dst_type: &str,
        mode: &ClientMode,
    ) -> eyre::Result<&dyn RelayPairPlugin> {
        let key = ChainPair::new(src_type, dst_type, mode.clone());
        if let Some(p) = self.pairs.get(&key) {
            return Ok(p.as_ref());
        }
        if *mode != ClientMode::Default {
            let fallback = ChainPair::new(src_type, dst_type, ClientMode::Default);
            if let Some(p) = self.pairs.get(&fallback) {
                return Ok(p.as_ref());
            }
        }
        eyre::bail!("unsupported relay pair: {src_type} -> {dst_type} (mode: {mode:?})")
    }

    pub fn client_builder(
        &self,
        src_type: &str,
        dst_type: &str,
        mode: &ClientMode,
    ) -> eyre::Result<&dyn ClientBuilder> {
        let key = ChainPair::new(src_type, dst_type, mode.clone());
        if let Some(b) = self.client_builders.get(&key) {
            return Ok(b.as_ref());
        }
        if *mode != ClientMode::Default {
            let fallback = ChainPair::new(src_type, dst_type, ClientMode::Default);
            if let Some(b) = self.client_builders.get(&fallback) {
                return Ok(b.as_ref());
            }
        }
        eyre::bail!("no client builder for {src_type} -> {dst_type} (mode: {mode:?})")
    }
}
