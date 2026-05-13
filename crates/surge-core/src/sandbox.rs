//! Sandbox configuration for nodes and profiles.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SandboxConfig {
    pub mode: SandboxMode,
    #[serde(default)]
    pub writable_roots: Vec<PathBuf>,
    #[serde(default)]
    pub network_allowlist: Vec<String>,
    #[serde(default)]
    pub shell_allowlist: Vec<String>,
    #[serde(default)]
    pub protected_paths: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: SandboxMode::WorkspaceWrite,
            writable_roots: Vec::new(),
            network_allowlist: Vec::new(),
            shell_allowlist: Vec::new(),
            protected_paths: Vec::new(),
        }
    }
}

/// Sandbox capability level requested by a node or profile.
///
/// Surge maps each variant to a runtime-specific set of launch flags via
/// [`crate::sandbox_matrix::RuntimeSandboxMatrix`]. Marked `#[non_exhaustive]`
/// so future capability levels (for example a dedicated `Networked` tier or
/// per-tool granular modes) can be added without breaking downstream matches.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    /// Read-only filesystem, no network, no shell.
    ReadOnly,
    /// Write within the workspace, no network.
    WorkspaceWrite,
    /// Write within the workspace plus network egress (subject to allowlist).
    WorkspaceNetwork,
    /// Unrestricted; the agent runtime owns all enforcement (or none).
    FullAccess,
    /// Caller-defined launch flags via [`SandboxConfig`] fields.
    Custom,
}

/// Reason `validate_custom` rejects a [`SandboxConfig`].
///
/// `Custom` mode delegates launch-flag composition to the caller; surge has
/// to refuse obviously-unsafe inputs (path escapes, shell metacharacters,
/// malformed host patterns) because they would flow straight to the agent
/// runtime without further checks.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SandboxValidationError {
    /// `mode = Custom` was declared, but every allowlist is empty — there is
    /// no signal for what the custom sandbox should permit.
    #[error("custom sandbox declared but every allowlist is empty")]
    CustomAllAllowlistsEmpty,
    /// A `writable_roots` entry contains a `..` segment.
    #[error("writable root `{path}` contains `..` segments")]
    WritableRootEscape {
        /// The offending path, surfaced verbatim for the diagnostic.
        path: String,
    },
    /// A `network_allowlist` entry is empty or contains whitespace, control
    /// characters, or characters that are not valid in a host pattern.
    #[error("network allowlist entry `{entry}` is not a valid host or IP pattern")]
    NetworkPatternInvalid {
        /// The offending entry, surfaced verbatim for the diagnostic.
        entry: String,
    },
    /// A `shell_allowlist` entry contains shell metacharacters (`;`, `|`,
    /// `&&`) that would let the agent chain commands past the allowlist.
    #[error("shell allowlist entry `{entry}` contains shell metacharacters")]
    ShellMetacharacters {
        /// The offending entry, surfaced verbatim for the diagnostic.
        entry: String,
    },
}

/// Validate `SandboxConfig::Custom` constraints.
///
/// Returns all violations rather than short-circuiting on the first — graph
/// validation is non-fail-fast and surfaces every problem in one pass.
///
/// Non-`Custom` configurations are always accepted by this validator. Their
/// launch-flag mapping comes from the bundled
/// [`crate::sandbox_matrix::RuntimeSandboxMatrix`] and is enforced by the
/// resolver in `surge-acp`, not here.
///
/// # Errors
///
/// Returns a non-empty `Vec` when one or more rules fail.
#[must_use]
pub fn validate_custom(cfg: &SandboxConfig) -> Vec<SandboxValidationError> {
    let mut errs = Vec::new();
    if cfg.mode != SandboxMode::Custom {
        return errs;
    }
    if cfg.writable_roots.is_empty()
        && cfg.network_allowlist.is_empty()
        && cfg.shell_allowlist.is_empty()
    {
        errs.push(SandboxValidationError::CustomAllAllowlistsEmpty);
    }
    for root in &cfg.writable_roots {
        // `..` parent-traversal segments are unsafe regardless of whether the
        // path is absolute or relative — the agent runtime resolves them at
        // launch and can escape the intended root.
        let raw = root.to_string_lossy();
        if raw.split(['/', '\\']).any(|seg| seg == "..") {
            errs.push(SandboxValidationError::WritableRootEscape {
                path: raw.into_owned(),
            });
        }
    }
    for entry in &cfg.network_allowlist {
        if !is_valid_host_pattern(entry) {
            errs.push(SandboxValidationError::NetworkPatternInvalid {
                entry: entry.clone(),
            });
        }
    }
    for entry in &cfg.shell_allowlist {
        if has_shell_metacharacters(entry) {
            errs.push(SandboxValidationError::ShellMetacharacters {
                entry: entry.clone(),
            });
        }
    }
    errs
}

/// `true` when the entry could plausibly be a host or IP pattern.
///
/// The rule is intentionally permissive — surge does not own the host-pattern
/// grammar of every runtime — but rejects entries that contain whitespace,
/// shell metacharacters, or are empty.
fn is_valid_host_pattern(entry: &str) -> bool {
    if entry.is_empty() {
        return false;
    }
    !entry
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, ';' | '|' | '&' | '`' | '$'))
}

/// `true` when the shell-allowlist entry contains characters that would let
/// the agent chain commands past the allowlist.
fn has_shell_metacharacters(entry: &str) -> bool {
    entry.contains(';') || entry.contains('|') || entry.contains("&&") || entry.contains('`')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_workspace_write() {
        let cfg = SandboxConfig::default();
        assert_eq!(cfg.mode, SandboxMode::WorkspaceWrite);
        assert!(cfg.network_allowlist.is_empty());
    }

    #[test]
    fn mode_serializes_kebab_case() {
        let json = serde_json::json!(SandboxMode::WorkspaceNetwork);
        assert_eq!(json, "workspace-network");
    }

    #[test]
    fn config_toml_roundtrip() {
        let original = SandboxConfig {
            mode: SandboxMode::WorkspaceWrite,
            writable_roots: vec![PathBuf::from("/tmp/work")],
            network_allowlist: vec!["crates.io".into()],
            shell_allowlist: vec!["cargo".into()],
            protected_paths: vec![".git".into()],
        };
        let toml_s = toml::to_string(&original).unwrap();
        let parsed: SandboxConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(original, parsed);
    }

    fn custom_with(writable: Vec<&str>, network: Vec<&str>, shell: Vec<&str>) -> SandboxConfig {
        SandboxConfig {
            mode: SandboxMode::Custom,
            writable_roots: writable.into_iter().map(PathBuf::from).collect(),
            network_allowlist: network.into_iter().map(String::from).collect(),
            shell_allowlist: shell.into_iter().map(String::from).collect(),
            protected_paths: Vec::new(),
        }
    }

    #[test]
    fn validate_custom_accepts_non_custom_modes() {
        for mode in [
            SandboxMode::ReadOnly,
            SandboxMode::WorkspaceWrite,
            SandboxMode::WorkspaceNetwork,
            SandboxMode::FullAccess,
        ] {
            let cfg = SandboxConfig {
                mode,
                ..Default::default()
            };
            assert!(
                validate_custom(&cfg).is_empty(),
                "non-Custom mode {mode:?} should never produce a custom-validator error",
            );
        }
    }

    #[test]
    fn validate_custom_rejects_empty_allowlists() {
        let cfg = SandboxConfig {
            mode: SandboxMode::Custom,
            ..Default::default()
        };
        let errs = validate_custom(&cfg);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0], SandboxValidationError::CustomAllAllowlistsEmpty);
    }

    #[test]
    fn validate_custom_accepts_any_non_empty_allowlist() {
        let cases = [
            custom_with(vec!["/tmp/work"], vec![], vec![]),
            custom_with(vec![], vec!["crates.io"], vec![]),
            custom_with(vec![], vec![], vec!["cargo"]),
        ];
        for cfg in cases {
            assert!(validate_custom(&cfg).is_empty(), "expected ok for {cfg:?}",);
        }
    }

    #[test]
    fn validate_custom_flags_writable_root_escape() {
        let cfg = custom_with(vec!["/tmp/work/../etc"], vec![], vec![]);
        let errs = validate_custom(&cfg);
        assert!(errs.iter().any(
            |e| matches!(e, SandboxValidationError::WritableRootEscape { path } if path.contains(".."))
        ));
    }

    #[test]
    fn validate_custom_flags_writable_root_escape_windows_separator() {
        let cfg = custom_with(vec![r"C:\work\..\etc"], vec![], vec![]);
        let errs = validate_custom(&cfg);
        assert!(
            errs.iter()
                .any(|e| matches!(e, SandboxValidationError::WritableRootEscape { .. }))
        );
    }

    #[test]
    fn validate_custom_flags_invalid_network_pattern() {
        let cases = [
            ("", "empty"),
            ("with space", "whitespace"),
            ("host;rm -rf /", "shell semicolon"),
            ("host | cat", "shell pipe"),
            ("host`whoami`", "shell backtick"),
        ];
        for (entry, label) in cases {
            let cfg = custom_with(vec![], vec![entry], vec![]);
            let errs = validate_custom(&cfg);
            assert!(
                errs.iter().any(|e| matches!(
                    e,
                    SandboxValidationError::NetworkPatternInvalid { entry: e2 } if e2 == entry
                )),
                "expected NetworkPatternInvalid for {label} ({entry:?})",
            );
        }
    }

    #[test]
    fn validate_custom_accepts_valid_network_patterns() {
        let cfg = custom_with(
            vec![],
            vec![
                "crates.io",
                "*.example.com",
                "127.0.0.1",
                "api.example.com:443",
            ],
            vec![],
        );
        assert!(validate_custom(&cfg).is_empty());
    }

    #[test]
    fn validate_custom_flags_shell_metacharacters() {
        let cases = ["cargo build && rm -rf .", "cargo|grep err", "cargo;ls"];
        for entry in cases {
            let cfg = custom_with(vec![], vec![], vec![entry]);
            let errs = validate_custom(&cfg);
            assert!(
                errs.iter().any(|e| matches!(
                    e,
                    SandboxValidationError::ShellMetacharacters { entry: e2 } if e2 == entry
                )),
                "expected ShellMetacharacters for {entry:?}",
            );
        }
    }

    #[test]
    fn validate_custom_accumulates_multiple_errors() {
        let cfg = custom_with(vec!["/tmp/../etc"], vec!["bad host"], vec!["cargo;ls"]);
        let errs = validate_custom(&cfg);
        assert_eq!(
            errs.len(),
            3,
            "expected three distinct violations, got {errs:?}",
        );
    }
}
