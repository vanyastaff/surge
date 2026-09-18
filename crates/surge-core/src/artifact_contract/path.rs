//! Path helpers for artifact contract path matching, and [`RelPath`] — the
//! one relative-path type Surge persists.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

pub(super) fn normalize_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// A path relative to a root Surge names elsewhere (the project root for
/// [`crate::run_event::EventPayload::ComposedArtifactInstalled`]), stored in
/// one canonical spelling.
///
/// Invariants, enforced at construction and therefore at deserialization:
/// - UTF-8, `/`-separated; a `\` anywhere is refused rather than treated
///   as a separator on one platform and a file-name character on another;
/// - not absolute: no leading `/` and no drive-style first segment
///   (`C:`, `C:foo`) — checked as text, so the answer does not depend on
///   the host's notion of a path prefix;
/// - no `..` segment (so it cannot escape its root);
/// - at least one real segment (`""`, `"."`, `"./"` name nothing).
///
/// `.` and empty segments are dropped and the rest re-joined with `/`, so
/// the durable form is byte-identical on every platform and two producers
/// naming the same file agree on the string. That agreement is what a
/// `(repo_id, rel_path)` trust-store key needs; a `PathBuf` compares by
/// component and serializes in the platform's own spelling, which is not
/// the same guarantee. Validation deliberately does not go through
/// [`std::path::Component`], whose splitting rules are platform-specific.
///
/// Serializes as the canonical string. Not a filesystem path by itself:
/// join it onto its root with [`RelPath::join_onto`] before touching disk.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelPath(String);

/// Why a string or path was refused as a [`RelPath`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RelPathError {
    /// The input is not valid UTF-8, so it has no canonical text form.
    #[error("path {0:?} is not UTF-8")]
    NotUtf8(String),
    /// The input contained `\`; only `/` separates segments.
    #[error("path {0:?} contains a backslash; use `/` as the separator")]
    Backslash(String),
    /// The input had a leading `/` or a drive-style first segment.
    #[error("path {0:?} is absolute; a relative path is required")]
    Absolute(String),
    /// The input contained a `..` segment.
    #[error("path {0:?} contains `..`; it could escape its root")]
    ParentDir(String),
    /// The input had no real segment left after dropping `.` and empties.
    #[error("path {0:?} names no file or directory")]
    Empty(String),
}

impl RelPath {
    /// Build from any path-like input, normalizing it.
    ///
    /// # Errors
    /// [`RelPathError`] when the input is not UTF-8, contains `\`, is
    /// absolute, contains `..`, or has no real segment.
    #[must_use = "the canonical path is the return value; the input is not modified"]
    pub fn new(path: impl AsRef<Path>) -> Result<Self, RelPathError> {
        let path = path.as_ref();
        let Some(text) = path.to_str() else {
            return Err(RelPathError::NotUtf8(path.display().to_string()));
        };
        Self::from_text(text)
    }

    fn from_text(text: &str) -> Result<Self, RelPathError> {
        let refused = || text.to_owned();
        if text.contains('\\') {
            return Err(RelPathError::Backslash(refused()));
        }
        if text.starts_with('/') || Self::starts_with_drive(text) {
            return Err(RelPathError::Absolute(refused()));
        }
        let mut parts: Vec<&str> = Vec::new();
        for segment in text.split('/') {
            match segment {
                "" | "." => {},
                ".." => return Err(RelPathError::ParentDir(refused())),
                normal => parts.push(normal),
            }
        }
        if parts.is_empty() {
            return Err(RelPathError::Empty(refused()));
        }
        Ok(Self(parts.join("/")))
    }

    /// `C:` / `C:foo` / `C:/foo` — a Windows drive spelling, refused on
    /// every platform so the decision is the same everywhere.
    fn starts_with_drive(text: &str) -> bool {
        let mut chars = text.chars();
        matches!(
            (chars.next(), chars.next()),
            (Some(drive), Some(':')) if drive.is_ascii_alphabetic()
        )
    }

    /// The canonical `/`-joined form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Resolve against `root`, producing a path the platform can open.
    #[must_use]
    pub fn join_onto(&self, root: &Path) -> PathBuf {
        self.0.split('/').fold(root.to_path_buf(), |mut acc, part| {
            acc.push(part);
            acc
        })
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelPath({:?})", self.0)
    }
}

impl FromStr for RelPath {
    type Err = RelPathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_text(s)
    }
}

impl TryFrom<&Path> for RelPath {
    type Error = RelPathError;

    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        Self::new(path)
    }
}

impl TryFrom<PathBuf> for RelPath {
    type Error = RelPathError;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        Self::new(path)
    }
}

impl AsRef<str> for RelPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Serialize for RelPath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RelPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

pub(super) fn is_adr_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("docs/adr/") else {
        return false;
    };
    if !rest.ends_with(".md") {
        return false;
    }
    let stem = rest.trim_end_matches(".md");
    let Some((number, slug)) = stem.split_once('-') else {
        return false;
    };
    number.len() == 4 && number.chars().all(|ch| ch.is_ascii_digit()) && !slug.is_empty()
}

pub(super) fn is_story_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("stories/story-") else {
        return false;
    };
    let Some(number) = rest.strip_suffix(".md") else {
        return false;
    };
    number.len() == 3 && number.chars().all(|ch| ch.is_ascii_digit())
}

/// `profiles/<name>.toml` where `<name>` is a single non-empty segment.
pub(super) fn is_profile_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("profiles/") else {
        return false;
    };
    let Some(name) = rest.strip_suffix(".toml") else {
        return false;
    };
    !name.is_empty() && !name.contains('/')
}

#[cfg(test)]
mod tests {
    use super::super::contract::{ArtifactKind, contract_for};
    use super::{RelPath, RelPathError};
    use std::path::Path;

    #[test]
    fn rel_path_normalizes_and_round_trips() {
        let p = RelPath::new("./.surge/./flows/bug-fix-1.0.toml").unwrap();
        assert_eq!(p.as_str(), ".surge/flows/bug-fix-1.0.toml");
        assert_eq!(p.to_string().parse::<RelPath>().unwrap(), p);
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\".surge/flows/bug-fix-1.0.toml\"");
        assert_eq!(serde_json::from_str::<RelPath>(&json).unwrap(), p);
        assert_eq!(
            p.join_onto(Path::new("/repo")),
            Path::new("/repo/.surge/flows/bug-fix-1.0.toml")
        );
    }

    #[test]
    fn rel_path_rejects_absolute_parent_and_empty() {
        let absolute = ["/etc/passwd", "C:foo", "C:/foo", "c:", "//srv/share"];
        for input in absolute {
            assert!(
                matches!(RelPath::new(input), Err(RelPathError::Absolute(_))),
                "{input:?} must be refused as absolute"
            );
        }
        assert!(matches!(
            RelPath::new("a/../b"),
            Err(RelPathError::ParentDir(_))
        ));
        assert!(matches!(
            RelPath::new(".."),
            Err(RelPathError::ParentDir(_))
        ));
        for input in ["", ".", "././"] {
            assert!(
                matches!(RelPath::new(input), Err(RelPathError::Empty(_))),
                "{input:?} must be refused as empty"
            );
        }
        // Deserialization goes through the same constructor.
        assert!(serde_json::from_str::<RelPath>("\"/abs\"").is_err());
        assert!(serde_json::from_str::<RelPath>("\"x/..\"").is_err());
    }

    #[test]
    fn rel_path_refuses_backslashes_on_every_platform() {
        // `\` is a separator on Windows and a file-name byte on Unix; a
        // key type that must agree across both refuses it outright rather
        // than letting the writing host decide what it meant.
        for input in ["a\\b", "\\srv\\share", ".surge\\flows\\x.toml"] {
            assert!(
                matches!(RelPath::new(input), Err(RelPathError::Backslash(_))),
                "{input:?} must be refused"
            );
        }
        // A colon that is not a drive spelling is an ordinary character;
        // a single letter before it is a drive on Windows and refused.
        assert_eq!(RelPath::new("ab:c/d e").unwrap().as_str(), "ab:c/d e");
        assert_eq!(RelPath::new("x/a:b").unwrap().as_str(), "x/a:b");
        assert!(matches!(
            RelPath::new("a:b"),
            Err(RelPathError::Absolute(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rel_path_refuses_non_utf8_instead_of_mangling() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let raw = OsStr::from_bytes(b"flows/\xff.toml");
        assert!(matches!(
            RelPath::new(Path::new(raw)),
            Err(RelPathError::NotUtf8(_))
        ));
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 512,
            ..Default::default()
        })]

        /// Acceptance law, with an oracle built from `std::path` plus the
        /// two text rules `std::path` cannot express portably — a different
        /// mechanism from the constructor's own `split('/')` walk. Also pins
        /// *which* error each rejection produces.
        #[test]
        fn rel_path_accepts_iff_relative_and_nonempty(
            s in "[a-zA-Z0-9_./:\\\\-]{0,24}"
        ) {
            use std::path::Component;
            let has_backslash = s.contains('\\');
            let drive = s.len() >= 2
                && s.as_bytes()[0].is_ascii_alphabetic()
                && s.as_bytes()[1] == b':';
            let components: Vec<_> = Path::new(&s).components().collect();
            let parent = components.iter().any(|c| matches!(c, Component::ParentDir));
            let rooted = s.starts_with('/');
            let has_normal = components.iter().any(|c| matches!(c, Component::Normal(_)));
            let expected = if has_backslash {
                Err("backslash")
            } else if rooted || drive {
                Err("absolute")
            } else if parent {
                Err("parent")
            } else if !has_normal {
                Err("empty")
            } else {
                Ok(())
            };
            let actual = match RelPath::new(&s) {
                Ok(_) => Ok(()),
                Err(RelPathError::Backslash(_)) => Err("backslash"),
                Err(RelPathError::Absolute(_)) => Err("absolute"),
                Err(RelPathError::ParentDir(_)) => Err("parent"),
                Err(RelPathError::Empty(_)) => Err("empty"),
                Err(other) => Err(Box::leak(format!("{other:?}").into_boxed_str()) as &str),
            };
            proptest::prop_assert_eq!(actual, expected, "input {:?}", s);
        }

        /// Canonical-form law: `Display` → `FromStr` is the identity, the
        /// stored form is already normalized (re-parsing changes nothing),
        /// and it is `/`-joined without `.` segments.
        #[test]
        fn rel_path_canonical_form_is_a_fixpoint(
            parts in proptest::collection::vec("[a-zA-Z0-9_-]{1,8}", 1..5),
            dots in proptest::collection::vec(proptest::bool::ANY, 1..5),
        ) {
            let noisy: Vec<String> = parts
                .iter()
                .zip(dots.iter().cycle())
                .map(|(p, dot)| if *dot { format!("./{p}") } else { p.clone() })
                .collect();
            let p = RelPath::new(noisy.join("/")).unwrap();
            proptest::prop_assert_eq!(p.as_str(), parts.join("/"));
            proptest::prop_assert_eq!(p.to_string().parse::<RelPath>().unwrap(), p.clone());
            proptest::prop_assert!(!p.as_str().split('/').any(|seg| seg == "." || seg.is_empty()));
        }
    }

    #[test]
    fn path_patterns_accept_expected_locations() {
        assert!(contract_for(ArtifactKind::Adr).accepts_path(Path::new("docs/adr/0001-choice.md")));
        assert!(contract_for(ArtifactKind::Story).accepts_path(Path::new("stories/story-001.md")));
        assert!(!contract_for(ArtifactKind::Story).accepts_path(Path::new("stories/story-1.md")));
        assert!(
            contract_for(ArtifactKind::Profile).accepts_path(Path::new("profiles/reviewer.toml"))
        );
        assert!(!contract_for(ArtifactKind::Profile).accepts_path(Path::new("profiles/a/b.toml")));
        assert!(!contract_for(ArtifactKind::Profile).accepts_path(Path::new("profiles/.toml")));
        assert!(!contract_for(ArtifactKind::Profile).accepts_path(Path::new("profile.toml")));
    }
}
