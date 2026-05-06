//! Shared types for `surge-intake`.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Identifier of an external ticket, formatted as `provider:scope#id`.
///
/// Examples:
/// - `"github_issues:user/repo#1234"`
/// - `"linear:wsp_acme/ABC-42"`
///
/// `TaskId` is opaque; the only operation supported is creation from a string
/// (via `try_new`) and serialization. Provider-specific parsing belongs to
/// the implementation, not to this type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(String);

impl TaskId {
    pub fn try_new(s: impl Into<String>) -> Result<Self, String> {
        let s = s.into();
        if s.is_empty() {
            return Err("task id must not be empty".into());
        }
        if !s.contains(':') {
            return Err(format!("task id must contain provider prefix: {s}"));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty() {
        assert!(TaskId::try_new("").is_err());
    }

    #[test]
    fn rejects_no_provider_prefix() {
        assert!(TaskId::try_new("just-a-string").is_err());
    }

    #[test]
    fn accepts_valid() {
        let id = TaskId::try_new("github_issues:user/repo#1234").unwrap();
        assert_eq!(id.as_str(), "github_issues:user/repo#1234");
    }

    #[test]
    fn round_trip_serde_json() {
        let id = TaskId::try_new("linear:wsp_acme/ABC-42").unwrap();
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(s, "\"linear:wsp_acme/ABC-42\"");
        let back: TaskId = serde_json::from_str(&s).unwrap();
        assert_eq!(back, id);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn round_trip(provider in "[a-z_]{3,15}", scope in "[a-zA-Z0-9_/-]{1,40}", num in 0u32..1_000_000) {
            let raw = format!("{provider}:{scope}#{num}");
            let id = TaskId::try_new(&raw).unwrap();
            let s = serde_json::to_string(&id).unwrap();
            let back: TaskId = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(back, id);
        }
    }
}
