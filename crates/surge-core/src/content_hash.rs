//! Content-addressed 32-byte hash with `sha256:hex` string representation.

use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn compute(content: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(content);
        let digest = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest);
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.to_hex())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContentHashParseError {
    #[error("expected `sha256:<64 hex chars>` or 64 hex chars, got {0:?}")]
    BadFormat(String),
    #[error("hex decode failed: {0}")]
    Hex(#[from] hex::FromHexError),
}

impl FromStr for ContentHash {
    type Err = ContentHashParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex_part = s.strip_prefix("sha256:").unwrap_or(s);
        if hex_part.len() != 64 {
            return Err(ContentHashParseError::BadFormat(s.to_string()));
        }
        let bytes = hex::decode(hex_part)?;
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }
}

impl serde::Serialize for ContentHash {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for ContentHash {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl schemars::JsonSchema for ContentHash {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ContentHash")
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("surge::content_hash::ContentHash")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "Content hash in the canonical `sha256:<64 hex chars>` form. The bare 64-char hex form is also accepted on parse.",
            "pattern": "^(sha256:)?[0-9a-fA-F]{64}$"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[test]
    fn compute_then_display_then_parse_roundtrip() {
        let h = ContentHash::compute(b"hello world");
        let s = h.to_string();
        assert!(s.starts_with("sha256:"));
        let parsed: ContentHash = s.parse().unwrap();
        assert_eq!(h, parsed);
    }

    #[test]
    fn deterministic_compute() {
        let a = ContentHash::compute(b"same input");
        let b = ContentHash::compute(b"same input");
        assert_eq!(a, b);
    }

    #[test]
    fn known_sha256_of_empty_input() {
        let h = ContentHash::compute(b"");
        assert_eq!(
            h.to_string(),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn parse_accepts_bare_hex() {
        let with_prefix: ContentHash =
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                .parse()
                .unwrap();
        let bare: ContentHash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            .parse()
            .unwrap();
        assert_eq!(with_prefix, bare);
    }

    #[test]
    fn parse_rejects_short_input() {
        let result: Result<ContentHash, _> = "sha256:abc".parse();
        assert!(result.is_err());
    }

    #[test]
    fn serde_roundtrip_via_toml() {
        #[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
        struct Wrapper {
            h: ContentHash,
        }

        let original = Wrapper {
            h: ContentHash::compute(b"test"),
        };
        let toml_s = toml::to_string(&original).unwrap();
        assert!(toml_s.contains("sha256:"));
        let parsed: Wrapper = toml::from_str(&toml_s).unwrap();
        assert_eq!(original, parsed);
    }
}
