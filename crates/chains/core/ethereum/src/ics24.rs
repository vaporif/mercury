use alloy::primitives::{B256, U256, keccak256};

const COMMITMENT_DISCRIMINATOR: u8 = 0x01;
const RECEIPT_DISCRIMINATOR: u8 = 0x02;
const ACK_DISCRIMINATOR: u8 = 0x03;

/// ERC-7201 base slot for `IBCStoreStorage`.
///
/// Computed as: `keccak256(abi.encode(uint256(keccak256("ibc.storage.IBCStore")) - 1)) & ~bytes32(uint256(0xff))`
/// See `IBCStoreUpgradeable.sol` for the derivation.
pub const IBC_STORE_COMMITMENTS_SLOT: U256 = U256::from_be_bytes([
    0x12, 0x60, 0x94, 0x44, 0x89, 0x27, 0x29, 0x88, 0xd9, 0xdf, 0x28, 0x51, 0x49, 0xb5, 0xaa, 0x1b,
    0x0f, 0x48, 0xf2, 0x13, 0x6d, 0x6f, 0x41, 0x61, 0x59, 0xf8, 0x40, 0xa3, 0xe0, 0x74, 0x76, 0x00,
]);

/// Build the ICS24 commitment path: `clientId || discriminator || sequence_be`.
fn ics24_path(client_id: &str, discriminator: u8, sequence: u64) -> Vec<u8> {
    let mut path = Vec::with_capacity(client_id.len() + 1 + 8);
    path.extend_from_slice(client_id.as_bytes());
    path.push(discriminator);
    path.extend_from_slice(&sequence.to_be_bytes());
    path
}

fn hashed_path(client_id: &str, discriminator: u8, sequence: u64) -> B256 {
    keccak256(ics24_path(client_id, discriminator, sequence))
}

#[must_use]
pub fn packet_commitment_key(client_id: &str, sequence: u64) -> B256 {
    hashed_path(client_id, COMMITMENT_DISCRIMINATOR, sequence)
}

#[must_use]
pub fn packet_receipt_key(client_id: &str, sequence: u64) -> B256 {
    hashed_path(client_id, RECEIPT_DISCRIMINATOR, sequence)
}

#[must_use]
pub fn ack_commitment_key(client_id: &str, sequence: u64) -> B256 {
    hashed_path(client_id, ACK_DISCRIMINATOR, sequence)
}

/// Compute the EVM storage slot for a `mapping(bytes32 => bytes32)` entry.
///
/// For Solidity mappings, the slot is `keccak256(abi.encode(key, baseSlot))`.
#[must_use]
pub fn commitment_storage_slot(hashed_path_key: B256) -> U256 {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(hashed_path_key.as_slice());
    buf[32..64].copy_from_slice(&IBC_STORE_COMMITMENTS_SLOT.to_be_bytes::<32>());
    U256::from_be_bytes(keccak256(buf).0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ics24_path_matches_solidity_encoding() {
        // ICS24Host: abi.encodePacked(clientId, uint8(1), uint64ToBigEndian(42))
        let path = ics24_path("07-tendermint-0", COMMITMENT_DISCRIMINATOR, 42);
        assert!(path.starts_with(b"07-tendermint-0"));
        assert_eq!(path[15], 0x01);
        assert_eq!(u64::from_be_bytes(path[16..24].try_into().unwrap()), 42);
    }

    #[test]
    fn different_discriminators_produce_different_keys() {
        let k1 = packet_commitment_key("client-0", 1);
        let k2 = packet_receipt_key("client-0", 1);
        let k3 = ack_commitment_key("client-0", 1);
        assert_ne!(k1, k2);
        assert_ne!(k2, k3);
        assert_ne!(k1, k3);
    }

    #[test]
    fn different_keys_produce_different_slots() {
        let k1 = packet_commitment_key("client-0", 1);
        let k2 = packet_commitment_key("client-0", 2);
        assert_ne!(commitment_storage_slot(k1), commitment_storage_slot(k2));
    }

    #[test]
    fn ibc_store_commitments_slot_matches_erc7201() {
        // ERC-7201: keccak256(abi.encode(uint256(keccak256("ibc.storage.IBCStore")) - 1)) & ~0xff
        let namespace_hash = keccak256(b"ibc.storage.IBCStore");
        let inner = U256::from_be_bytes(namespace_hash.0) - U256::from(1);
        let mut abi_encoded = [0u8; 32];
        abi_encoded.copy_from_slice(&inner.to_be_bytes::<32>());
        let slot = U256::from_be_bytes(keccak256(abi_encoded).0);
        let mask = !U256::from(0xff);
        assert_eq!(slot & mask, IBC_STORE_COMMITMENTS_SLOT);
    }
}
