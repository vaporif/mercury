use std::collections::HashMap;

use crate::plugin::{ChainPlugin, RelayPairPlugin};

#[derive(Default)]
pub struct ChainRegistry {
    chains: HashMap<&'static str, Box<dyn ChainPlugin>>,
    pairs: HashMap<(&'static str, &'static str), Box<dyn RelayPairPlugin>>,
}

impl ChainRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::path::Path;
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::*;
    use crate::plugin::{
        AnyChain, AnyClientId, ChainId, ChainStatusInfo, ClientStateInfo, DynRelay,
    };

    struct Named(&'static str);

    #[async_trait]
    impl ChainPlugin for Named {
        fn chain_type(&self) -> &'static str {
            self.0
        }
        fn validate_config(&self, _raw: &toml::Table) -> eyre::Result<()> {
            Ok(())
        }
        async fn connect(&self, _raw: &toml::Table, _dir: &Path) -> eyre::Result<AnyChain> {
            eyre::bail!("offline")
        }
        fn parse_client_id(&self, raw: &str) -> eyre::Result<AnyClientId> {
            Ok(Box::new(raw.to_string()))
        }
        async fn query_status(&self, _chain: &AnyChain) -> eyre::Result<ChainStatusInfo> {
            eyre::bail!("offline")
        }
        fn chain_id_from_config(&self, _raw: &toml::Table) -> eyre::Result<ChainId> {
            Ok(ChainId::from(self.0))
        }
        fn rpc_addr_from_config(&self, _raw: &toml::Table) -> eyre::Result<String> {
            Ok(String::new())
        }
        async fn build_create_client_payload(
            &self,
            _chain: &AnyChain,
        ) -> eyre::Result<Box<dyn Any + Send + Sync>> {
            eyre::bail!("offline")
        }
        async fn create_client(
            &self,
            _chain: &AnyChain,
            _payload: Box<dyn Any + Send + Sync>,
        ) -> eyre::Result<String> {
            eyre::bail!("offline")
        }
        async fn query_client_state_info(
            &self,
            _chain: &AnyChain,
            _client_id: &str,
            _height: Option<u64>,
        ) -> eyre::Result<ClientStateInfo> {
            eyre::bail!("offline")
        }
        async fn query_commitment_sequences(
            &self,
            _chain: &AnyChain,
            _client_id: &str,
            _height: Option<u64>,
        ) -> eyre::Result<Vec<u64>> {
            eyre::bail!("offline")
        }
        async fn build_update_client_payload(
            &self,
            _chain: &AnyChain,
            _trusted_height: u64,
            _target_height: u64,
            _counterparty_client_state: Option<&(dyn Any + Send + Sync)>,
        ) -> eyre::Result<Box<dyn Any + Send + Sync>> {
            eyre::bail!("offline")
        }
        async fn update_client(
            &self,
            _chain: &AnyChain,
            _client_id: &str,
            _payload: Box<dyn Any + Send + Sync>,
        ) -> eyre::Result<()> {
            eyre::bail!("offline")
        }
    }

    struct Pair(&'static str, &'static str);

    impl RelayPairPlugin for Pair {
        fn src_type(&self) -> &'static str {
            self.0
        }
        fn dst_type(&self) -> &'static str {
            self.1
        }
        fn build_relay(
            &self,
            _src: &AnyChain,
            _dst: &AnyChain,
            _src_client_id: &AnyClientId,
            _dst_client_id: &AnyClientId,
        ) -> eyre::Result<(Arc<dyn DynRelay>, Arc<dyn DynRelay>)> {
            eyre::bail!("{}->{}", self.0, self.1)
        }
    }

    fn build_error(registry: &ChainRegistry, src: &str, dst: &str) -> String {
        let chain: AnyChain = Arc::new(());
        let id: AnyClientId = Box::new(());
        registry
            .pair(src, dst)
            .unwrap()
            .build_relay(&chain, &chain, &id, &id)
            .err()
            .unwrap()
            .to_string()
    }

    #[test]
    fn chain_lookup_returns_plugin_registered_under_its_type() {
        let mut registry = ChainRegistry::new();
        registry.register_chain(Named("cosmos"));
        registry.register_chain(Named("ethereum"));
        assert_eq!(registry.chain("cosmos").unwrap().chain_type(), "cosmos");
        assert_eq!(registry.chain("ethereum").unwrap().chain_type(), "ethereum");
    }

    #[test]
    fn unknown_chain_type_is_an_error_naming_the_type() {
        let registry = ChainRegistry::new();
        let err = registry.chain("near").err().unwrap().to_string();
        assert_eq!(err, "unsupported chain type 'near'");
    }

    #[test]
    fn pair_lookup_is_directional() {
        let mut registry = ChainRegistry::new();
        registry.register_pair(Pair("cosmos", "ethereum"));
        assert_eq!(
            build_error(&registry, "cosmos", "ethereum"),
            "cosmos->ethereum"
        );
        let err = registry
            .pair("ethereum", "cosmos")
            .err()
            .unwrap()
            .to_string();
        assert_eq!(err, "unsupported relay pair: ethereum -> cosmos");

        registry.register_pair(Pair("ethereum", "cosmos"));
        assert_eq!(
            build_error(&registry, "ethereum", "cosmos"),
            "ethereum->cosmos"
        );
    }
}
