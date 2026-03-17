use mercury_core::error::Result;

use mercury_cosmos::chain::CosmosChainInner;
use mercury_cosmos::config::CosmosChainConfig;
use mercury_cosmos::keys::CosmosSigner;

/// Wrapper around `CosmosChainInner` that is local to this crate,
/// enabling cross-chain trait impls without orphan rule violations.
#[derive(Clone, Debug)]
pub struct CosmosChain<S: CosmosSigner>(pub CosmosChainInner<S>);

impl<S: CosmosSigner> CosmosChain<S> {
    pub async fn new(config: CosmosChainConfig, signer: S) -> Result<Self> {
        CosmosChainInner::new(config, signer).await.map(Self)
    }
}

mercury_chain_traits::delegate_chain_inner! {
    impl[S: CosmosSigner] CosmosChain<S> => CosmosChainInner<S>
}
