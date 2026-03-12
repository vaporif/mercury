use std::fmt;
use std::path::Path;

use async_trait::async_trait;
use mercury_core::ThreadSafe;
use mercury_core::error::Result;
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde::Deserialize;
use zeroize::Zeroize;

#[derive(Deserialize)]
struct CosmosKeyFile {
    secret_key: String,
}

/// Load a Cosmos signer from a TOML key file.
///
/// The key file must contain a `secret_key` field with a hex-encoded
/// secp256k1 secret key.
pub fn load_cosmos_signer(key_file: &Path, account_prefix: &str) -> Result<Secp256k1KeyPair> {
    use mercury_core::error::WrapErr;

    let content = std::fs::read_to_string(key_file)
        .wrap_err_with(|| format!("reading key file {}", key_file.display()))?;
    let key_toml: CosmosKeyFile = toml::from_str(&content)
        .wrap_err_with(|| format!("parsing key file {}", key_file.display()))?;
    let mut secret_bytes = hex::decode(&key_toml.secret_key).wrap_err("decoding secret key hex")?;
    let mut secret_arr: [u8; 32] = secret_bytes
        .as_slice()
        .try_into()
        .map_err(|_| eyre::eyre!("secret key must be exactly 32 bytes"))?;
    secret_bytes.zeroize();
    let result = SecretKey::from_byte_array(secret_arr).wrap_err("invalid secret key");
    secret_arr.zeroize();
    let secret_key = result?;
    Ok(Secp256k1KeyPair::from_secret_key(
        secret_key,
        account_prefix,
    ))
}

/// Trait for Cosmos transaction signing backends.
///
/// Implementations can range from in-memory keys to HSM devices or cloud KMS.
/// `sign` is async to support I/O-bound backends (PKCS#11, AWS KMS, etc.).
#[async_trait]
pub trait CosmosSigner: ThreadSafe + fmt::Debug + Clone {
    /// Sign a SHA-256 digest and return the compact ECDSA signature bytes.
    async fn sign(&self, digest: [u8; 32]) -> Result<Vec<u8>>;

    /// The compressed secp256k1 public key (33 bytes).
    fn public_key_bytes(&self) -> Vec<u8>;

    /// The bech32 account address.
    fn account_address(&self) -> Result<String>;
}

/// An in-memory secp256k1 signing key pair with bech32 address derivation.
#[derive(Clone)]
pub struct Secp256k1KeyPair {
    secret_key: SecretKey,
    pub public_key: PublicKey,
    pub account_prefix: String,
}

impl fmt::Debug for Secp256k1KeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secp256k1KeyPair")
            .field("public_key", &self.public_key)
            .field("account_prefix", &self.account_prefix)
            .finish_non_exhaustive()
    }
}

impl Secp256k1KeyPair {
    /// Create a key pair from a raw secret key and bech32 account prefix.
    #[must_use]
    pub fn from_secret_key(secret_key: SecretKey, account_prefix: &str) -> Self {
        let secp = Secp256k1::new();
        let public_key = PublicKey::from_secret_key(&secp, &secret_key);
        Self {
            secret_key,
            public_key,
            account_prefix: account_prefix.to_string(),
        }
    }
}

#[async_trait]
impl CosmosSigner for Secp256k1KeyPair {
    async fn sign(&self, digest: [u8; 32]) -> Result<Vec<u8>> {
        let secp = Secp256k1::signing_only();
        let msg = secp256k1::Message::from_digest(digest);
        let sig = secp.sign_ecdsa(msg, &self.secret_key);
        Ok(sig.serialize_compact().to_vec())
    }

    fn public_key_bytes(&self) -> Vec<u8> {
        self.public_key.serialize().to_vec()
    }

    fn account_address(&self) -> Result<String> {
        use bech32::Hrp;
        use ripemd::Ripemd160;
        use sha2::{Digest, Sha256};
        let pub_key_bytes = self.public_key.serialize();
        let sha_hash = Sha256::digest(pub_key_bytes);
        let address_bytes = Ripemd160::digest(sha_hash);
        let hrp = Hrp::parse(&self.account_prefix)?;
        let encoded = bech32::encode::<bech32::Bech32>(hrp, &address_bytes)?;
        Ok(encoded)
    }
}
