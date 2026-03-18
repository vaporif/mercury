use mercury_core::error::Result;

use mercury_solana::chain::SolanaChain;
use mercury_solana::config::SolanaChainConfig;

#[derive(Clone, Debug)]
pub struct SolanaAdapter(pub SolanaChain);

impl SolanaAdapter {
    pub fn new(config: SolanaChainConfig) -> Result<Self> {
        SolanaChain::new(config).map(Self)
    }
}

mercury_chain_traits::delegate_chain! {
    impl[] SolanaAdapter => SolanaChain
}
