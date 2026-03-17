use mercury_core::error::Result;

use mercury_ethereum::chain::EthereumChainInner;
use mercury_ethereum::config::EthereumChainConfig;

/// Wrapper around `EthereumChainInner` that is local to this crate,
/// enabling cross-chain trait impls without orphan rule violations.
#[derive(Clone, Debug)]
pub struct EthereumChain(pub EthereumChainInner);

impl EthereumChain {
    pub async fn new(
        config: EthereumChainConfig,
        signer: alloy::signers::local::PrivateKeySigner,
    ) -> Result<Self> {
        EthereumChainInner::new(config, signer).await.map(Self)
    }
}

mercury_chain_traits::delegate_chain_inner! {
    impl[] EthereumChain => EthereumChainInner
}
