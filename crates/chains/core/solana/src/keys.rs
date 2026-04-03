use std::path::Path;

use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;

pub fn load_keypair(path: &Path) -> eyre::Result<Keypair> {
    let data = std::fs::read_to_string(path)
        .map_err(|e| eyre::eyre!("failed to read keypair file {}: {e}", path.display()))?;
    let bytes: Vec<u8> = serde_json::from_str(&data)
        .map_err(|e| eyre::eyre!("invalid keypair JSON in {}: {e}", path.display()))?;
    eyre::ensure!(
        bytes.len() == 64,
        "keypair must be 64 bytes, got {}",
        bytes.len()
    );
    let secret: [u8; 32] = bytes[..32]
        .try_into()
        .map_err(|_| eyre::eyre!("failed to extract secret key bytes"))?;
    let kp = Keypair::new_from_array(secret);
    let expected_pubkey = &bytes[32..64];
    eyre::ensure!(
        kp.pubkey().as_ref() == expected_pubkey,
        "derived public key does not match keypair file (file may be corrupted)"
    );
    Ok(kp)
}
