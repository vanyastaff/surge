//! Runtime identification for ACP agents (Claude Code, Codex, Gemini, …).
//!
//! [`RuntimeKind`] is the closed set of agent runtimes whose sandbox model
//! surge tracks in [`crate::sandbox_matrix::RuntimeSandboxMatrix`]. Adding a
//! new variant requires also adding at least one matrix row in
//! `crates/surge-core/bundled/sandbox/matrix.toml`; the matrix property test
//! asserts no `verified = true` row carries empty flags.

use serde::{Deserialize, Serialize};
use std::fmt;

const BUNDLED_VERSIONS_TOML: &str = include_str!("../bundled/sandbox/versions.toml");

/// Closed enumeration of ACP-conformant agent runtimes surge knows about.
///
/// The wire form (used in TOML, JSON, and `surge doctor` output) is the
/// `as_str()` rendering — that string is also the value emitted by `serde`.
/// `Custom` runtimes are out of scope: per-run custom launch flags live on
/// [`crate::sandbox::SandboxConfig`] when `mode = Custom`, not as their own
/// runtime variant.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuntimeKind {
    /// Anthropic Claude Code (`claude` binary, native ACP).
    #[serde(rename = "claude-code")]
    ClaudeCode,
    /// OpenAI Codex CLI (`codex` binary, ACP adapter).
    #[serde(rename = "codex")]
    Codex,
    /// Google Gemini CLI (`gemini` binary).
    #[serde(rename = "gemini")]
    Gemini,
    /// Cursor CLI (`cursor` binary, ACP adapter).
    #[serde(rename = "cursor")]
    CursorCli,
    /// GitHub Copilot CLI (`gh copilot` / `copilot`, public-preview ACP).
    #[serde(rename = "copilot")]
    CopilotCli,
    /// SST OpenCode CLI (`opencode` binary).
    #[serde(rename = "opencode")]
    OpenCode,
    /// Block Goose CLI (`goose` binary).
    #[serde(rename = "goose")]
    Goose,
}

impl RuntimeKind {
    /// Stable lowercase identifier for this runtime.
    ///
    /// Used as the on-disk key in `matrix.toml`, in event payloads, and in
    /// `surge doctor` text output. The returned string matches the value
    /// produced by `serde::Serialize`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::CursorCli => "cursor",
            Self::CopilotCli => "copilot",
            Self::OpenCode => "opencode",
            Self::Goose => "goose",
        }
    }
}

impl fmt::Display for RuntimeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Per-runtime minimum-version expectation declared by surge.
///
/// `surge doctor` and the engine consult this table at session-open time and
/// emit `warn` when the detected binary is older than `min_version`. Mismatch
/// is **warn-only** — surge does not refuse to launch on a stale binary.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeVersionPolicy {
    /// Runtime this policy applies to.
    pub runtime: RuntimeKind,
    /// Minimum semver-compatible version surge tested against.
    pub min_version: semver::VersionReq,
    /// Short rationale for the bump. Helps reviewers decide when to update.
    #[serde(default)]
    pub note: String,
}

impl RuntimeVersionPolicy {
    /// Constructor for tests and callers outside this crate.
    ///
    /// Struct-literal syntax does not compile across crate boundaries
    /// because the type is `#[non_exhaustive]`; this builder lets callers
    /// set the required fields and defaults the rest.
    #[must_use]
    pub fn new(runtime: RuntimeKind, min_version: semver::VersionReq) -> Self {
        Self {
            runtime,
            min_version,
            note: String::new(),
        }
    }

    /// Builder: attach an explanatory note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct BundledVersionsDocument {
    policies: Vec<RuntimeVersionPolicy>,
}

/// Look up the declared minimum-version policy for a runtime.
///
/// Returns `None` when surge has not declared a minimum (typical for
/// declared-but-unverified runtimes).
///
/// # Panics
///
/// Panics at startup if `bundled/sandbox/versions.toml` fails to parse —
/// it is a compile-time invariant of the crate.
#[must_use]
pub fn version_policy(runtime: RuntimeKind) -> Option<RuntimeVersionPolicy> {
    let doc: BundledVersionsDocument = toml::from_str(BUNDLED_VERSIONS_TOML)
        .expect("bundled sandbox versions.toml must parse — repo invariant");
    doc.policies.into_iter().find(|p| p.runtime == runtime)
}

/// All declared version policies, in declaration order from `versions.toml`.
#[must_use]
pub fn all_version_policies() -> Vec<RuntimeVersionPolicy> {
    let doc: BundledVersionsDocument = toml::from_str(BUNDLED_VERSIONS_TOML)
        .expect("bundled sandbox versions.toml must parse — repo invariant");
    doc.policies
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[RuntimeKind] = &[
        RuntimeKind::ClaudeCode,
        RuntimeKind::Codex,
        RuntimeKind::Gemini,
        RuntimeKind::CursorCli,
        RuntimeKind::CopilotCli,
        RuntimeKind::OpenCode,
        RuntimeKind::Goose,
    ];

    #[test]
    fn as_str_matches_display_for_every_variant() {
        for &kind in ALL {
            assert_eq!(format!("{kind}"), kind.as_str());
        }
    }

    #[test]
    fn serde_roundtrip_via_json() {
        for &kind in ALL {
            let s = serde_json::to_string(&kind).unwrap();
            let back: RuntimeKind = serde_json::from_str(&s).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn serde_wire_form_matches_as_str() {
        for &kind in ALL {
            let v = serde_json::to_value(kind).unwrap();
            assert_eq!(v, serde_json::Value::String(kind.as_str().to_owned()));
        }
    }

    #[test]
    fn distinct_wire_forms() {
        let mut seen = std::collections::BTreeSet::new();
        for &kind in ALL {
            assert!(
                seen.insert(kind.as_str()),
                "duplicate wire form for {kind:?}",
            );
        }
    }

    #[test]
    fn bundled_version_policies_parse() {
        let policies = all_version_policies();
        assert!(
            !policies.is_empty(),
            "bundled versions.toml must declare at least one policy",
        );
    }

    #[test]
    fn version_policy_returns_none_for_undeclared_runtime() {
        // Goose ships declared-unverified in matrix.toml; surge does not yet
        // pin a minimum version for it. If that changes, update this test.
        assert!(version_policy(RuntimeKind::Goose).is_none());
    }

    #[test]
    fn version_policy_returns_some_for_verified_runtimes() {
        for kind in [
            RuntimeKind::ClaudeCode,
            RuntimeKind::Codex,
            RuntimeKind::Gemini,
        ] {
            assert!(
                version_policy(kind).is_some(),
                "version policy must exist for verified runtime {kind:?}",
            );
        }
    }

    #[test]
    fn version_policy_notes_are_non_empty() {
        for p in all_version_policies() {
            assert!(
                !p.note.is_empty(),
                "policy for {:?} must carry a non-empty note explaining the floor",
                p.runtime,
            );
        }
    }
}
