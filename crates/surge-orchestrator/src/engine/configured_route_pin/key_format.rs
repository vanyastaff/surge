//! Shared durable codec; platform publication must not change this format.
use super::RoutePinError;
use sha2::{Digest, Sha256};
pub(super) const MAGIC: &[u8] = b"surge/route-pin-key/v1\0";
pub(super) const KEY_FILE_LEN: usize = MAGIC.len() + 64;
pub(super) fn checksum(key: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"surge/route-pin-key-integrity/v1\0");
    hash.update(MAGIC);
    hash.update(key);
    hash.finalize().into()
}
pub(super) fn decode(stored: &[u8]) -> Result<[u8; 32], RoutePinError> {
    if stored.len() != KEY_FILE_LEN || !stored.starts_with(MAGIC) {
        return Err(RoutePinError::InvalidKey);
    }
    let key: [u8; 32] = stored[MAGIC.len()..MAGIC.len() + 32]
        .try_into()
        .map_err(|_| RoutePinError::InvalidKey)?;
    if stored[MAGIC.len() + 32..] != checksum(&key) {
        return Err(RoutePinError::InvalidKey);
    }
    Ok(key)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durable_codec_rejects_same_size_corruption_and_extra_bytes() {
        let key = [42; 32];
        assert_eq!(
            checksum(&key),
            [
                0xb9, 0xba, 0x7e, 0xa0, 0x80, 0x86, 0x90, 0xd3, 0xf7, 0x57, 0xcd, 0x42, 0x2c, 0xe6,
                0x0f, 0x84, 0xa8, 0xca, 0xdd, 0x2a, 0x43, 0x89, 0x96, 0x07, 0x0f, 0x1a, 0xf9, 0xd7,
                0x4a, 0x52, 0xf3, 0x33
            ]
        );
        let mut encoded = MAGIC.to_vec();
        encoded.extend_from_slice(&key);
        encoded.extend_from_slice(&checksum(&key));
        assert_eq!(decode(&encoded).unwrap(), key);
        for offset in [0, MAGIC.len(), encoded.len() - 1] {
            let mut corrupt = encoded.clone();
            corrupt[offset] ^= 1;
            assert_eq!(decode(&corrupt), Err(RoutePinError::InvalidKey));
        }
        encoded.push(0);
        assert_eq!(decode(&encoded), Err(RoutePinError::InvalidKey));
    }
}
