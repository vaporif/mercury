use mercury_core::error::{Error, Result};
use secp256k1::{PublicKey, Secp256k1, SecretKey};

#[derive(Clone, Debug)]
pub struct Secp256k1KeyPair {
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
    pub account_prefix: String,
}

impl Secp256k1KeyPair {
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

    pub fn account_address(&self) -> Result<String> {
        use bech32::Hrp;
        use sha2::{Digest, Sha256};
        let pub_key_bytes = self.public_key.serialize();
        let sha_hash = Sha256::digest(pub_key_bytes);
        let address_bytes = &sha_hash[..20];
        let hrp = Hrp::parse(&self.account_prefix).map_err(Error::report)?;
        let encoded =
            bech32::encode::<bech32::Bech32>(hrp, address_bytes).map_err(Error::report)?;
        Ok(encoded)
    }
}
