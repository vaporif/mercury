use mercury_core::error::Result;

use mercury_cosmos::chain::CosmosChain;
use mercury_cosmos::config::CosmosChainConfig;
use mercury_cosmos::keys::CosmosSigner;

/// Wrapper around `CosmosChain` that is local to this crate,
/// enabling cross-chain trait impls without orphan rule violations.
#[derive(Clone, Debug)]
pub struct CosmosAdapter<S: CosmosSigner>(pub CosmosChain<S>);

impl<S: CosmosSigner> CosmosAdapter<S> {
    pub async fn new(config: CosmosChainConfig, signer: S) -> Result<Self> {
        CosmosChain::new(config, signer).await.map(Self)
    }
}

mercury_chain_traits::delegate_chain! {
    impl[S: CosmosSigner] CosmosAdapter<S> => CosmosChain<S>
}
