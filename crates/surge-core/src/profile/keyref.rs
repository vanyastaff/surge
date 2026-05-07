//! Parse profile references of the form `name@MAJOR.MINOR[.PATCH]`.
//!
//! `ProfileKey` (a newtype in [`crate::keys`]) only enforces character set;
//! it does not split the `name` and `version` halves. The registry needs
//! that split when resolving a reference like `implementer@1.0` against
//! the disk and bundled stores. This module owns the parser.
//!
//! ## Grammar
//!
//! ```text
//! key_ref     ::= name [ "@" version ]
//! name        ::= ASCII letter, then ASCII letter | digit | '_' | '-' | '.'
//! version     ::= MAJOR ( "." MINOR ( "." PATCH )? )?
//! ```
//!
//! Examples:
//! - `"implementer"`        → name only, no version constraint
//! - `"implementer@1"`      → name + major-only version (treated as `1.0.0`)
//! - `"implementer@1.0"`    → name + major.minor (treated as `1.0.0`)
//! - `"implementer@1.0.3"`  → name + full semver
//!
//! Anything else is a parse error.

use semver::Version;

use crate::keys::ProfileKey;

/// Errors returned by [`parse_key_ref`].
///
/// `Clone + PartialEq + Eq` are intentionally not derived because the
/// upstream `semver::Error` does not implement them; the `error_message`
/// field carries a `String` rendering of the underlying parse failure
/// instead.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum KeyRefParseError {
    /// The input string was empty.
    #[error("profile key reference is empty")]
    Empty,

    /// The reference contained more than one `@` separator.
    #[error("profile key reference has more than one '@': {0:?}")]
    TooManyAtSigns(String),

    /// The name portion (before `@`) was empty.
    #[error("profile key reference has empty name portion: {0:?}")]
    EmptyName(String),

    /// The name portion did not satisfy `ProfileKey` validation.
    #[error("invalid profile name {name:?}: {source}")]
    InvalidName {
        name: String,
        #[source]
        source: crate::keys::KeyParseError,
    },

    /// The version portion (after `@`) was empty.
    #[error("profile key reference has empty version portion: {0:?}")]
    EmptyVersion(String),

    /// The version portion was not a parseable semver-compatible version.
    #[error("invalid version {version:?} in {input:?}: {error_message}")]
    InvalidVersion {
        input: String,
        version: String,
        error_message: String,
    },
}

/// A parsed profile reference: a name plus an optional version.
///
/// This is intentionally separate from [`ProfileKey`]. `ProfileKey` is the
/// "wire" form used in `flow.toml` and on disk; `ProfileKeyRef` is the
/// "split" form the registry uses internally for resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileKeyRef {
    /// The name portion (before `@`), guaranteed to satisfy `ProfileKey`
    /// character-set rules.
    pub name: ProfileKey,
    /// `Some(version)` if the reference included `@MAJOR.MINOR[.PATCH]`,
    /// otherwise `None` (meaning "latest available").
    pub version: Option<Version>,
}

impl ProfileKeyRef {
    /// Construct a name-only reference (no version constraint).
    ///
    /// # Errors
    /// Propagates [`crate::keys::KeyParseError`] if `name` does not satisfy
    /// `ProfileKey` validation.
    pub fn name_only(name: impl Into<String>) -> Result<Self, KeyRefParseError> {
        let name_str = name.into();
        let key =
            ProfileKey::try_new(&name_str).map_err(|source| KeyRefParseError::InvalidName {
                name: name_str.clone(),
                source,
            })?;
        Ok(Self {
            name: key,
            version: None,
        })
    }
}

/// Parse a profile reference of the form `name` or `name@MAJOR[.MINOR[.PATCH]]`.
///
/// Returns the split form [`ProfileKeyRef`].
///
/// # Errors
/// Returns [`KeyRefParseError`] when the input is empty, contains more than
/// one `@`, has an empty name or version, has an invalid name (per
/// `ProfileKey` rules), or has a version that is not parseable as semver.
pub fn parse_key_ref(input: &str) -> Result<ProfileKeyRef, KeyRefParseError> {
    if input.is_empty() {
        tracing::warn!(target: "profile::keyref", "rejected empty profile key reference");
        return Err(KeyRefParseError::Empty);
    }

    let mut parts = input.splitn(3, '@');
    let name_part = parts.next().expect("splitn(3) yields at least one item");
    let version_part = parts.next();
    if parts.next().is_some() {
        tracing::warn!(target: "profile::keyref", input, "rejected key ref with multiple '@' separators");
        return Err(KeyRefParseError::TooManyAtSigns(input.to_string()));
    }

    if name_part.is_empty() {
        return Err(KeyRefParseError::EmptyName(input.to_string()));
    }
    let name = ProfileKey::try_new(name_part).map_err(|source| KeyRefParseError::InvalidName {
        name: name_part.to_string(),
        source,
    })?;

    let version = match version_part {
        None => None,
        Some("") => return Err(KeyRefParseError::EmptyVersion(input.to_string())),
        Some(v) => Some(parse_partial_semver(input, v)?),
    };

    tracing::debug!(
        target: "profile::keyref",
        name = name.as_str(),
        version = ?version,
        "parsed profile key reference"
    );

    Ok(ProfileKeyRef { name, version })
}

/// Accept `1`, `1.0`, or `1.0.3` and lift them to a full `semver::Version`
/// by zero-filling the missing positions.
fn parse_partial_semver(input: &str, raw: &str) -> Result<Version, KeyRefParseError> {
    let segments: Vec<&str> = raw.split('.').collect();
    if segments.len() > 3 || segments.iter().any(|s| s.is_empty()) {
        return Err(KeyRefParseError::InvalidVersion {
            input: input.to_string(),
            version: raw.to_string(),
            error_message: format!("malformed version segment(s) in {raw:?}"),
        });
    }
    let normalized: String = match segments.len() {
        1 => format!("{}.0.0", segments[0]),
        2 => format!("{}.{}.0", segments[0], segments[1]),
        3 => raw.to_string(),
        _ => unreachable!("len > 3 already returned above"),
    };
    Version::parse(&normalized).map_err(|source| KeyRefParseError::InvalidVersion {
        input: input.to_string(),
        version: raw.to_string(),
        error_message: source.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_only() {
        let r = parse_key_ref("implementer").unwrap();
        assert_eq!(r.name.as_str(), "implementer");
        assert!(r.version.is_none());
    }

    #[test]
    fn name_at_full_version() {
        let r = parse_key_ref("implementer@1.0.3").unwrap();
        assert_eq!(r.name.as_str(), "implementer");
        assert_eq!(r.version, Some(Version::new(1, 0, 3)));
    }

    #[test]
    fn name_at_major_minor_lifts_to_full() {
        let r = parse_key_ref("implementer@1.0").unwrap();
        assert_eq!(r.version, Some(Version::new(1, 0, 0)));
    }

    #[test]
    fn name_at_major_only_lifts_to_full() {
        let r = parse_key_ref("implementer@2").unwrap();
        assert_eq!(r.version, Some(Version::new(2, 0, 0)));
    }

    #[test]
    fn dashed_name_with_version() {
        let r = parse_key_ref("rust-impl@1.2.0").unwrap();
        assert_eq!(r.name.as_str(), "rust-impl");
        assert_eq!(r.version, Some(Version::new(1, 2, 0)));
    }

    #[test]
    fn dotted_name_with_version() {
        let r = parse_key_ref("acme.implementer@1.0.0").unwrap();
        assert_eq!(r.name.as_str(), "acme.implementer");
    }

    #[test]
    fn empty_input_rejected() {
        let e = parse_key_ref("").unwrap_err();
        assert!(matches!(e, KeyRefParseError::Empty));
    }

    #[test]
    fn empty_version_rejected() {
        let e = parse_key_ref("implementer@").unwrap_err();
        assert!(matches!(e, KeyRefParseError::EmptyVersion(_)));
    }

    #[test]
    fn empty_name_rejected() {
        let e = parse_key_ref("@1.0").unwrap_err();
        assert!(matches!(e, KeyRefParseError::EmptyName(_)));
    }

    #[test]
    fn double_at_rejected() {
        let e = parse_key_ref("implementer@1.0@extra").unwrap_err();
        assert!(matches!(e, KeyRefParseError::TooManyAtSigns(_)));
    }

    #[test]
    fn space_in_name_rejected() {
        let e = parse_key_ref("foo bar@1.0").unwrap_err();
        assert!(matches!(e, KeyRefParseError::InvalidName { .. }));
    }

    #[test]
    fn leading_digit_in_name_rejected() {
        let e = parse_key_ref("1foo@1.0").unwrap_err();
        assert!(matches!(e, KeyRefParseError::InvalidName { .. }));
    }

    #[test]
    fn unparseable_version_rejected() {
        let e = parse_key_ref("implementer@notaver").unwrap_err();
        assert!(matches!(e, KeyRefParseError::InvalidVersion { .. }));
    }

    #[test]
    fn version_with_too_many_dots_rejected() {
        let e = parse_key_ref("implementer@1.0.0.0").unwrap_err();
        assert!(matches!(e, KeyRefParseError::InvalidVersion { .. }));
    }

    #[test]
    fn name_only_constructor() {
        let r = ProfileKeyRef::name_only("implementer").unwrap();
        assert_eq!(r.name.as_str(), "implementer");
        assert!(r.version.is_none());
    }

    #[test]
    fn name_only_constructor_validates() {
        let e = ProfileKeyRef::name_only("1foo").unwrap_err();
        assert!(matches!(e, KeyRefParseError::InvalidName { .. }));
    }
}
