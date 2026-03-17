use std::path::Path;

use alloy::signers::local::PrivateKeySigner;
use zeroize::Zeroizing;

pub fn load_ethereum_signer(path: &Path) -> eyre::Result<PrivateKeySigner> {
    let content = Zeroizing::new(
        std::fs::read_to_string(path)
            .map_err(|e| eyre::eyre!("reading key file {}: {e}", path.display()))?,
    );
    parse_signer(&content, path)
}

fn parse_signer(content: &str, path: &Path) -> eyre::Result<PrivateKeySigner> {
    let hex_str = content
        .trim()
        .strip_prefix("0x")
        .unwrap_or_else(|| content.trim());
    hex_str
        .parse()
        .map_err(|e| eyre::eyre!("parsing private key from {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_signer_from_hex_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            tmp,
            "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
        )
        .unwrap();

        let signer = load_ethereum_signer(tmp.path()).unwrap();
        assert_eq!(
            signer.address().to_string().to_lowercase(),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }

    #[test]
    fn load_signer_with_0x_prefix() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            tmp,
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
        )
        .unwrap();

        let signer = load_ethereum_signer(tmp.path()).unwrap();
        assert_eq!(
            signer.address().to_string().to_lowercase(),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }

    #[test]
    fn load_signer_bad_file() {
        let result = load_ethereum_signer(Path::new("/nonexistent/key.hex"));
        assert!(result.is_err());
    }
}
