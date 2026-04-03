use mercury_core::error::Result;

use mercury_solana::chain::SolanaChain;
use mercury_solana::config::SolanaChainConfig;

#[derive(Clone, Debug)]
pub struct SolanaAdapter(pub SolanaChain);

impl SolanaAdapter {
    pub fn new(config: SolanaChainConfig) -> Result<Self> {
        SolanaChain::new(config).map(Self)
    }

    pub async fn new_and_init(config: SolanaChainConfig) -> Result<Self> {
        let mut chain = SolanaChain::new(config)?;
        chain.load_alt_cache().await?;
        Ok(Self(chain))
    }
}

mercury_chain_traits::delegate_chain! {
    impl[] SolanaAdapter => SolanaChain
}
