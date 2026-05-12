//! Stable string identifiers for Surge graph entities.
//!
//! These keys are *user-typed strings* in `flow.toml` (e.g., `"impl_2"`,
//! `"done"`, `"implementer@1.0"`). For *runtime-generated* IDs (`RunId`,
//! `SessionId`) see [`crate::id`].
//!
//! Each key type lives in a distinct Rust newtype, so cross-type assignment
//! is a compile error. Validation enforces character-set and length rules
//! at construction time and at deserialization time.

/// Errors produced when parsing a key string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyParseError {
    #[error("key is empty")]
    Empty,
    #[error("key too long: {len} chars (max {max})")]
    TooLong { len: usize, max: usize },
    #[error("invalid character {ch:?} at position {pos}")]
    InvalidChar { ch: char, pos: usize },
    #[error("key must start with ASCII letter, got {ch:?}")]
    InvalidStart { ch: char },
}

/// Validate that `s` contains only ASCII alphanumeric plus characters in
/// `extras`, starts with an ASCII letter, and is within `max_len`.
///
/// This is the helper that backs every key type's constructor. It is `pub`
/// so test code and key-using crates can validate strings without
/// constructing a key.
pub fn validate_key_chars(s: &str, max_len: usize, extras: &[u8]) -> Result<(), KeyParseError> {
    if s.is_empty() {
        return Err(KeyParseError::Empty);
    }
    if s.len() > max_len {
        return Err(KeyParseError::TooLong {
            len: s.len(),
            max: max_len,
        });
    }
    let first = s.chars().next().expect("non-empty checked above");
    if !first.is_ascii_alphabetic() {
        return Err(KeyParseError::InvalidStart { ch: first });
    }
    for (pos, ch) in s.char_indices() {
        let ok = ch.is_ascii_alphanumeric() || (ch.is_ascii() && extras.contains(&(ch as u8)));
        if !ok {
            return Err(KeyParseError::InvalidChar { ch, pos });
        }
    }
    Ok(())
}

/// Define a string-newtype key with character-set validation, custom
/// serde, `Display`, `FromStr`, and `TryFrom<String>`/`TryFrom<&str>`.
///
/// Usage: `define_key!(NodeKey, max_len = 32, extras = b"_");`
macro_rules! define_key {
    ($name:ident, max_len = $max:expr, extras = $extras:expr $(,)?) => {
        #[doc = concat!("Stable string identifier (max ", stringify!($max), " chars).")]
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// Maximum allowed length in characters.
            pub const MAX_LEN: usize = $max;

            /// Construct from any string-like input, validating character set and length.
            ///
            /// # Errors
            /// Returns [`KeyParseError`] if the input is empty, too long, contains
            /// disallowed characters, or does not start with an ASCII letter.
            pub fn try_new(s: impl Into<String>) -> Result<Self, $crate::keys::KeyParseError> {
                let s = s.into();
                $crate::keys::validate_key_chars(&s, Self::MAX_LEN, $extras)?;
                Ok(Self(s))
            }

            /// View as `&str`.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume into the underlying `String`.
            #[must_use]
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl ::std::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                write!(f, "{}({:?})", stringify!($name), self.0)
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = $crate::keys::KeyParseError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::try_new(s)
            }
        }

        impl<'a> TryFrom<&'a str> for $name {
            type Error = $crate::keys::KeyParseError;
            fn try_from(s: &'a str) -> Result<Self, Self::Error> {
                Self::try_new(s)
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::keys::KeyParseError;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                Self::try_new(s)
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                Self::try_new(s).map_err(::serde::de::Error::custom)
            }
        }

        impl ::schemars::JsonSchema for $name {
            fn schema_name() -> ::std::borrow::Cow<'static, str> {
                ::std::borrow::Cow::Borrowed(stringify!($name))
            }

            fn schema_id() -> ::std::borrow::Cow<'static, str> {
                ::std::borrow::Cow::Borrowed(concat!("surge::keys::", stringify!($name)))
            }

            fn json_schema(_generator: &mut ::schemars::SchemaGenerator) -> ::schemars::Schema {
                ::schemars::json_schema!({
                    "type": "string",
                    "description": concat!(
                        stringify!($name),
                        ": validated identifier (ASCII alphanumeric plus configured extras, starts with a letter, max ",
                        stringify!($max),
                        " chars)."
                    ),
                    "minLength": 1,
                    "maxLength": $max
                })
            }
        }
    };
}

// ── Public key types ────────────────────────────────────────────────

// Strict charset (alphanumeric + underscore, leading letter), max 32 chars.
define_key!(NodeKey, max_len = 32, extras = b"_");
define_key!(EdgeKey, max_len = 32, extras = b"_");
define_key!(OutcomeKey, max_len = 32, extras = b"_");
define_key!(SubgraphKey, max_len = 32, extras = b"_");

// Extended charset (alphanumeric + `_-.@`), max 64 chars. Allows
// `"implementer@1.0"`, `"rust-crate-tdd@1.2.3"`.
define_key!(ProfileKey, max_len = 64, extras = b"_-.@");
define_key!(TemplateKey, max_len = 64, extras = b"_-.@");

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    // ── Round-trip / parse positives ────────────────────────────────

    #[test]
    fn node_key_accepts_valid_string() {
        let key = NodeKey::try_from("impl_2").unwrap();
        assert_eq!(key.as_str(), "impl_2");
    }

    #[test]
    fn profile_key_accepts_versioned_form() {
        let key = ProfileKey::try_from("implementer@1.0").unwrap();
        assert_eq!(key.as_str(), "implementer@1.0");
    }

    #[test]
    fn template_key_accepts_dashed_versioned_form() {
        let key = TemplateKey::try_from("rust-crate-tdd@1.2.3").unwrap();
        assert_eq!(key.as_str(), "rust-crate-tdd@1.2.3");
    }

    // ── Length and start-char negatives ─────────────────────────────

    #[test]
    fn node_key_rejects_too_long() {
        let too_long = "a".repeat(33);
        let err = NodeKey::try_from(too_long.as_str()).unwrap_err();
        assert!(matches!(err, KeyParseError::TooLong { len: 33, max: 32 }));
    }

    #[test]
    fn node_key_rejects_empty() {
        let err = NodeKey::try_from("").unwrap_err();
        assert_eq!(err, KeyParseError::Empty);
    }

    #[test]
    fn node_key_rejects_leading_digit() {
        let err = NodeKey::try_from("1foo").unwrap_err();
        assert!(matches!(err, KeyParseError::InvalidStart { ch: '1' }));
    }

    #[test]
    fn node_key_rejects_at_sign() {
        // NodeKey uses strict charset — `@` is not allowed.
        let err = NodeKey::try_from("foo@bar").unwrap_err();
        assert!(matches!(err, KeyParseError::InvalidChar { ch: '@', .. }));
    }

    #[test]
    fn node_key_rejects_dash() {
        // NodeKey uses strict charset — `-` is not allowed.
        let err = NodeKey::try_from("foo-bar").unwrap_err();
        assert!(matches!(err, KeyParseError::InvalidChar { ch: '-', .. }));
    }

    #[test]
    fn profile_key_rejects_space() {
        let err = ProfileKey::try_from("foo bar").unwrap_err();
        assert!(matches!(err, KeyParseError::InvalidChar { ch: ' ', .. }));
    }

    // ── Type-distinctness ───────────────────────────────────────────

    #[test]
    fn outcome_key_and_node_key_are_distinct_types() {
        let _node = NodeKey::try_from("foo").unwrap();
        let _outcome = OutcomeKey::try_from("foo").unwrap();
        // Compile error if you uncomment: let _x: NodeKey = _outcome;
    }

    // ── Display / Debug ─────────────────────────────────────────────

    #[test]
    fn display_emits_inner_string() {
        let key = NodeKey::try_from("impl_2").unwrap();
        assert_eq!(format!("{key}"), "impl_2");
    }

    #[test]
    fn debug_includes_type_name() {
        let key = ProfileKey::try_from("implementer@1.0").unwrap();
        let s = format!("{key:?}");
        assert!(s.starts_with("ProfileKey("), "got: {s}");
        assert!(s.contains("implementer@1.0"));
    }

    // ── Serde round-trips ───────────────────────────────────────────

    #[test]
    fn toml_roundtrip_in_struct() {
        // The key fix: deserializing from TOML works because our custom
        // `Deserialize` impl asks for an *owned* `String`, not a `&'de str`.
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Holder {
            id: NodeKey,
        }
        let original = Holder {
            id: NodeKey::try_from("spec_1").unwrap(),
        };
        let toml_s = toml::to_string(&original).unwrap();
        assert_eq!(toml_s, "id = \"spec_1\"\n");
        let parsed: Holder = toml::from_str(&toml_s).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn json_roundtrip_in_struct() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Holder {
            profile: ProfileKey,
        }
        let original = Holder {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: Holder = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn deserialize_validates_invalid_string() {
        // Invalid char `@` in NodeKey context — must error at deserialize time.
        let toml_s = "id = \"impl@2\"\n";
        #[derive(Deserialize)]
        struct Holder {
            #[allow(dead_code)]
            id: NodeKey,
        }
        let result: Result<Holder, _> = toml::from_str(toml_s);
        assert!(result.is_err(), "expected deser to reject invalid char");
    }

    #[test]
    fn deserialize_validates_too_long() {
        let too_long = "a".repeat(33);
        let toml_s = format!("id = \"{too_long}\"\n");
        #[derive(Deserialize)]
        struct Holder {
            #[allow(dead_code)]
            id: NodeKey,
        }
        let result: Result<Holder, _> = toml::from_str(&toml_s);
        assert!(result.is_err());
    }

    // ── FromStr ─────────────────────────────────────────────────────

    #[test]
    fn from_str_parses_valid() {
        let key: NodeKey = "valid_id".parse().unwrap();
        assert_eq!(key.as_str(), "valid_id");
    }

    #[test]
    fn from_str_rejects_invalid() {
        let result: Result<NodeKey, _> = "1invalid".parse();
        assert!(result.is_err());
    }
}
