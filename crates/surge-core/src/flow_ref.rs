//! Flow template references of the form `name@MAJOR[.MINOR]`.
//!
//! A [`FlowRef`] names a flow template family in the flow catalog
//! (`.surge/flows/` → `SURGE_HOME/flows/` → bundled archetypes) at *major*
//! granularity, optionally pinned to a minor. It is deliberately **not**
//! [`semver::Version`]: the catalog's templates are versioned by their
//! metadata, and a task pinning `bug-fix@1` means "the best 1.x template",
//! not "the file `bug-fix-1.0.0.toml`". There is no patch position — a
//! pinned template is a family, not a build.
//!
//! ## Grammar
//!
//! ```text
//! flow_ref ::= name "@" MAJOR [ "." MINOR ]
//! name     ::= ASCII letter, then ASCII letter | digit | '_' | '-'
//! MAJOR    ::= decimal digits, no leading zero
//! MINOR    ::= decimal digits, no leading zero
//! ```
//!
//! Unlike [`crate::profile::keyref`], the version is **required**: a bare
//! `bug-fix` ("latest of anything") has no meaning for dispatch, where an
//! unresolvable template is a named error and never a fallback.
//!
//! Examples: `bug-fix@1`, `linear-with-review@2.1`.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::keys::{KeyParseError, validate_key_chars};

/// Longest accepted template name.
const MAX_NAME_LEN: usize = 64;

/// Why a string was refused as a [`FlowRef`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FlowRefParseError {
    /// The input was empty.
    #[error("flow reference is empty")]
    Empty,
    /// The reference contained more than one `@`.
    #[error("flow reference has more than one '@': {0:?}")]
    TooManyAtSigns(String),
    /// The name portion (before `@`) was empty.
    #[error("flow reference has empty name portion: {0:?}")]
    EmptyName(String),
    /// The name portion did not satisfy key character-set rules.
    #[error("invalid flow name {name:?}: {source}")]
    InvalidName {
        /// The offending name portion.
        name: String,
        /// The underlying character-set failure.
        #[source]
        source: KeyParseError,
    },
    /// The `@` was present but no version followed it.
    #[error("flow reference has empty version portion: {0:?}")]
    EmptyVersion(String),
    /// The version portion had no major, or a segment was not canonical.
    #[error("invalid flow version {version:?} in {input:?}")]
    InvalidVersion {
        /// The full input that failed.
        input: String,
        /// The version portion that failed.
        version: String,
    },
}

/// A parsed flow template reference: name, major, optional minor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FlowRef {
    name: String,
    major: u64,
    minor: Option<u64>,
}

impl FlowRef {
    /// Construct from parts, validating the name.
    ///
    /// # Errors
    /// [`FlowRefParseError::InvalidName`] when `name` violates the
    /// character-set rules.
    pub fn new(
        name: impl Into<String>,
        major: u64,
        minor: Option<u64>,
    ) -> Result<Self, FlowRefParseError> {
        let name = name.into();
        validate_key_chars(&name, MAX_NAME_LEN, b"_-").map_err(|source| {
            FlowRefParseError::InvalidName {
                name: name.clone(),
                source,
            }
        })?;
        Ok(Self { name, major, minor })
    }

    /// The template name (`bug-fix`).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The pinned major version.
    #[must_use]
    pub fn major(&self) -> u64 {
        self.major
    }

    /// The pinned minor version, if any.
    #[must_use]
    pub fn minor(&self) -> Option<u64> {
        self.minor
    }

    /// Whether a catalog entry versioned `version` satisfies this reference:
    /// same major, and same minor when one is pinned.
    #[must_use]
    pub fn matches(&self, version: &semver::Version) -> bool {
        version.major == self.major && self.minor.is_none_or(|minor| version.minor == minor)
    }
}

impl fmt::Display for FlowRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.minor {
            Some(minor) => write!(f, "{}@{}.{}", self.name, self.major, minor),
            None => write!(f, "{}@{}", self.name, self.major),
        }
    }
}

impl FromStr for FlowRef {
    type Err = FlowRefParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.is_empty() {
            return Err(FlowRefParseError::Empty);
        }
        let mut parts = input.splitn(3, '@');
        let name = parts.next().unwrap_or_default();
        let version = parts.next();
        if parts.next().is_some() {
            return Err(FlowRefParseError::TooManyAtSigns(input.to_string()));
        }
        if name.is_empty() {
            return Err(FlowRefParseError::EmptyName(input.to_string()));
        }
        validate_key_chars(name, MAX_NAME_LEN, b"_-").map_err(|source| {
            FlowRefParseError::InvalidName {
                name: name.to_string(),
                source,
            }
        })?;
        let Some(version) = version else {
            return Err(FlowRefParseError::EmptyVersion(input.to_string()));
        };
        if version.is_empty() {
            return Err(FlowRefParseError::EmptyVersion(input.to_string()));
        }
        let mut segments = version.split('.');
        let major = parse_canonical_number(segments.next().unwrap_or_default(), input, version)?;
        let minor = match segments.next() {
            Some(segment) => Some(parse_canonical_number(segment, input, version)?),
            None => None,
        };
        if segments.next().is_some() {
            return Err(FlowRefParseError::InvalidVersion {
                input: input.to_string(),
                version: version.to_string(),
            });
        }
        Ok(Self {
            name: name.to_string(),
            major,
            minor,
        })
    }
}

/// Parse one version segment, refusing empty strings, non-digits and
/// leading zeros (so `Display` ∘ `FromStr` is the identity on valid input).
fn parse_canonical_number(
    segment: &str,
    input: &str,
    version: &str,
) -> Result<u64, FlowRefParseError> {
    if segment.is_empty() || !segment.bytes().all(|b| b.is_ascii_digit()) {
        return Err(FlowRefParseError::InvalidVersion {
            input: input.to_string(),
            version: version.to_string(),
        });
    }
    if segment.len() > 1 && segment.starts_with('0') {
        return Err(FlowRefParseError::InvalidVersion {
            input: input.to_string(),
            version: version.to_string(),
        });
    }
    segment
        .parse::<u64>()
        .map_err(|_| FlowRefParseError::InvalidVersion {
            input: input.to_string(),
            version: version.to_string(),
        })
}

impl Serialize for FlowRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for FlowRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for FlowRef {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("FlowRef")
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("surge::flow_ref::FlowRef")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "Flow template reference `name@MAJOR[.MINOR]` (e.g. `bug-fix@1`). The version is required; an unresolvable reference is an error, never a fallback.",
            "pattern": r"^[A-Za-z][A-Za-z0-9_-]*@[0-9]+(\.[0-9]+)?$"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> FlowRef {
        s.parse().unwrap()
    }

    #[test]
    fn major_only_parses() {
        let r = parse("bug-fix@1");
        assert_eq!(r.name(), "bug-fix");
        assert_eq!(r.major(), 1);
        assert_eq!(r.minor(), None);
    }

    #[test]
    fn major_minor_parses() {
        let r = parse("linear-with-review@2.1");
        assert_eq!(r.major(), 2);
        assert_eq!(r.minor(), Some(1));
    }

    #[test]
    fn display_round_trips_through_parse() {
        for s in ["bug-fix@1", "linear-3@2.0", "a@10", "x@1.9"] {
            assert_eq!(parse(s).to_string(), s);
        }
    }

    #[test]
    fn rejects_bare_name() {
        let e = "bug-fix".parse::<FlowRef>().unwrap_err();
        assert!(matches!(e, FlowRefParseError::EmptyVersion(_)));
    }

    #[test]
    fn rejects_empty_input() {
        let e = "".parse::<FlowRef>().unwrap_err();
        assert!(matches!(e, FlowRefParseError::Empty));
    }

    #[test]
    fn rejects_empty_parts() {
        assert!(matches!(
            "@1".parse::<FlowRef>().unwrap_err(),
            FlowRefParseError::EmptyName(_)
        ));
        assert!(matches!(
            "bug-fix@".parse::<FlowRef>().unwrap_err(),
            FlowRefParseError::EmptyVersion(_)
        ));
    }

    #[test]
    fn rejects_double_at_and_patch() {
        assert!(matches!(
            "a@1@2".parse::<FlowRef>().unwrap_err(),
            FlowRefParseError::TooManyAtSigns(_)
        ));
        assert!(matches!(
            "a@1.0.0".parse::<FlowRef>().unwrap_err(),
            FlowRefParseError::InvalidVersion { .. }
        ));
    }

    #[test]
    fn rejects_non_canonical_numbers() {
        for s in ["a@01", "a@1.", "a@1.02", "a@x", "a@1.x"] {
            assert!(
                matches!(
                    s.parse::<FlowRef>().unwrap_err(),
                    FlowRefParseError::InvalidVersion { .. }
                ),
                "{s} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_invalid_names() {
        for s in ["-x@1", "1x@1", "x y@1", "x.y@1"] {
            assert!(
                matches!(
                    s.parse::<FlowRef>().unwrap_err(),
                    FlowRefParseError::InvalidName { .. }
                ),
                "{s} should be rejected"
            );
        }
    }

    #[test]
    fn matches_major_and_minor() {
        let major_only = parse("bug-fix@1");
        assert!(major_only.matches(&semver::Version::new(1, 0, 0)));
        assert!(major_only.matches(&semver::Version::new(1, 7, 3)));
        assert!(!major_only.matches(&semver::Version::new(2, 0, 0)));

        let pinned = parse("bug-fix@1.2");
        assert!(pinned.matches(&semver::Version::new(1, 2, 9)));
        assert!(!pinned.matches(&semver::Version::new(1, 3, 0)));
    }

    #[test]
    fn serde_is_the_string_form() {
        let r = parse("bug-fix@1.2");
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "\"bug-fix@1.2\"");
        let back: FlowRef = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn new_validates_name() {
        assert!(FlowRef::new("bug-fix", 1, None).is_ok());
        assert!(matches!(
            FlowRef::new("bad name", 1, None).unwrap_err(),
            FlowRefParseError::InvalidName { .. }
        ));
    }
}
