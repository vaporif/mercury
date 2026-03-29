use async_trait::async_trait;
use eyre::Result;

/// Material needed to sign transactions and derive addresses.
#[derive(Clone, Debug)]
pub struct Wallet {
    /// Hex-encoded 32-byte secp256k1 secret key.
    pub secret_key_hex: String,
    /// Bech32 account address.
    pub address: String,
    /// Human-readable name (e.g., "relayer", "user1").
    pub name: String,
}

/// What a running chain exposes to tests.
pub trait ChainHandle: Send + Sync {
    fn rpc_endpoint(&self) -> &str;
    fn grpc_endpoint(&self) -> &str;
    fn chain_id(&self) -> &str;
    fn relayer_wallet(&self) -> &Wallet;
    fn user_wallets(&self) -> &[Wallet];
}

/// Abstracts how a test chain gets started.
#[async_trait(?Send)]
pub trait ChainBootstrap {
    type Handle: ChainHandle;

    async fn start(&self) -> Result<Self::Handle>;
}
