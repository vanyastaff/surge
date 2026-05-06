//! Type-safe identifiers for Surge entities.

use serde::{Deserialize, Serialize};
use std::fmt;
use ulid::Ulid;

macro_rules! define_id {
    ($name:ident, $prefix:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(Ulid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Ulid::new())
            }

            #[must_use]
            pub fn as_ulid(&self) -> Ulid {
                self.0
            }

            /// Short form for human-facing UI: first 12 chars of the ULID, no prefix.
            ///
            /// 12 chars = 10 timestamp chars + 2 randomness chars (~1024 distinct
            /// suffixes per millisecond). Used in branch names, worktree paths, log
            /// lines. Callers that absolutely require uniqueness use the full ID.
            #[must_use]
            pub fn short(&self) -> String {
                let s = self.0.to_string();
                debug_assert_eq!(s.len(), 26, "ULID display is always 26 chars");
                s[..12].to_string()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}-{}", $prefix, self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = ulid::DecodeError;

            /// Parse from either the prefixed form (`"spec-01ARZ3NDEKTSV4RRFFQ69G5FAV"`)
            /// or the bare ULID string (`"01ARZ3NDEKTSV4RRFFQ69G5FAV"`).
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let ulid_str = s.strip_prefix(concat!($prefix, "-")).unwrap_or(s);
                ulid_str.parse::<Ulid>().map(Self)
            }
        }
    };
}

define_id!(SpecId, "spec");
define_id!(TaskId, "task");
define_id!(SubtaskId, "sub");

// New runtime IDs added in M1 for Surge data model.
define_id!(RunId, "run");
define_id!(SessionId, "session");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let a = SpecId::new();
        let b = SpecId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn display_has_prefix() {
        let id = SpecId::new();
        let s = id.to_string();
        assert!(s.starts_with("spec-"));
    }

    #[test]
    fn from_str_prefixed_roundtrip() {
        let id = TaskId::new();
        let s = id.to_string(); // "task-01ARZ..."
        let parsed: TaskId = s.parse().unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn from_str_bare_ulid() {
        let id = SubtaskId::new();
        let bare = id.as_ulid().to_string(); // no prefix
        let parsed: SubtaskId = bare.parse().unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn from_str_wrong_prefix_still_parses_as_bare() {
        // "task-<ulid>" parsed as SpecId strips nothing (wrong prefix),
        // so it tries to parse "task-<ulid>" as a raw ULID and fails.
        let task_id = TaskId::new();
        let task_str = task_id.to_string(); // "task-01ARZ..."
        let result: Result<SpecId, _> = task_str.parse();
        assert!(result.is_err());
    }

    #[test]
    fn from_str_invalid_returns_error() {
        let result: Result<SpecId, _> = "not-a-valid-id".parse();
        assert!(result.is_err());
    }

    #[test]
    fn run_id_displays_with_prefix() {
        let id = RunId::new();
        assert!(id.to_string().starts_with("run-"));
    }

    #[test]
    fn session_id_displays_with_prefix() {
        let id = SessionId::new();
        assert!(id.to_string().starts_with("session-"));
    }

    #[test]
    fn run_id_roundtrips_via_string() {
        let id = RunId::new();
        let s = id.to_string();
        let parsed: RunId = s.parse().unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn run_id_and_session_id_are_distinct_types() {
        let r = RunId::new();
        let s = r.to_string();
        // Cross-type parse must fail because prefix differs.
        let result: Result<SessionId, _> = s.parse();
        assert!(result.is_err());
    }

    #[test]
    fn short_is_12_chars_for_all_ids() {
        assert_eq!(SpecId::new().short().len(), 12);
        assert_eq!(TaskId::new().short().len(), 12);
        assert_eq!(SubtaskId::new().short().len(), 12);
        assert_eq!(RunId::new().short().len(), 12);
        assert_eq!(SessionId::new().short().len(), 12);
    }

    #[test]
    fn short_is_alphanumeric_and_distinct() {
        let a = RunId::new();
        let b = RunId::new();
        let sa = a.short();
        let sb = b.short();
        assert_eq!(sa.len(), 12);
        assert_eq!(sb.len(), 12);
        assert!(sa.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(sb.chars().all(|c| c.is_ascii_alphanumeric()));
        // Note: we deliberately don't assert that adjacent ULIDs share a timestamp
        // prefix. On fast hardware, two `Ulid::new()` calls back-to-back can land
        // on different millisecond boundaries, which makes any prefix-equality
        // assertion racy (observed flaking on macOS CI).
        // Randomness chars (positions 10..12) almost always differ — collision is ~1/1024.
        assert_ne!(
            sa, sb,
            "two distinct ULIDs should produce distinct short forms"
        );
    }
}
