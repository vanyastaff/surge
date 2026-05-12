//! Structured report types for `surge doctor`.
//!
//! The CLI command lives in `surge-cli`; this module owns the data shape only
//! so `surge-core` stays the single source of truth for what a doctor report
//! contains. Adapters (CLI / UI / JSON-RPC) render these types into whatever
//! format the caller wants.

use crate::runtime::{RuntimeKind, RuntimeVersionPolicy};
use crate::sandbox::SandboxMode;
use serde::{Deserialize, Serialize};

/// Aggregate report produced by `surge doctor`.
///
/// Entries are sorted in declaration order from the registry; this is the
/// order operators see in text output and the order materialized in
/// machine-readable JSON / TOML.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DoctorReport {
    /// One entry per detected ACP agent (or per declared registry entry, when
    /// the agent was not found on PATH).
    pub entries: Vec<DoctorEntry>,
}

/// Per-agent slice of a [`DoctorReport`].
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorEntry {
    /// Registry name (e.g. `"claude-acp"`, `"codex-acp"`).
    pub agent_name: String,
    /// Runtime kind surge maps this agent to. `None` when the registry entry
    /// has no `runtime` field (legacy registries) — `surge doctor` reports
    /// these as "matrix lookup skipped".
    pub runtime: Option<RuntimeKind>,
    /// Path to the binary on the user's machine, if detected.
    ///
    /// `None` means the registry declares the agent but the binary is not on
    /// `PATH`. `surge doctor` surfaces these with a "not detected" status.
    pub binary_path: Option<String>,
    /// Detected version. `None` when the binary was not found or the
    /// `<binary> --version` probe failed.
    pub detected_version: Option<String>,
    /// Declared minimum-version policy for the runtime, if surge tracks one.
    pub policy: Option<RuntimeVersionPolicy>,
    /// Result of comparing `detected_version` against `policy.min_version`.
    pub version_status: VersionStatus,
    /// Matrix-row status per `SandboxMode`, in canonical mode order
    /// (read-only, workspace-write, workspace-network, full-access). `Custom`
    /// is intentionally excluded from this view (handled by per-config
    /// validation, not by matrix lookup).
    pub matrix: Vec<MatrixCell>,
}

/// Outcome of comparing a detected runtime version against the declared
/// minimum-version policy.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VersionStatus {
    /// No version policy declared — nothing to compare.
    NotApplicable,
    /// `--version` probe failed; surge could not determine the version.
    ProbeFailed,
    /// Detected version satisfies the policy.
    Ok,
    /// Detected version is below the declared minimum (warn-only).
    BelowMinimum,
}

/// Matrix lookup result for a single `(runtime, mode)` pair.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatrixCell {
    /// Sandbox mode this cell represents.
    pub mode: SandboxMode,
    /// Cell status — verified / declared-unverified / unsupported.
    pub status: MatrixCellStatus,
    /// Launch flags surge would emit for this `(runtime, mode)` pair when
    /// `status = Verified`. Empty otherwise.
    #[serde(default)]
    pub flags: Vec<String>,
    /// Note from the matrix row (rationale, upstream link). Empty when no
    /// note is set.
    #[serde(default)]
    pub note: String,
}

/// Status of a matrix cell as seen by `surge doctor`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatrixCellStatus {
    /// Matrix declares a row with `verified = true` and concrete flags.
    Verified,
    /// Matrix declares a row but `verified = false` — surge has not tested
    /// the live runtime against this mode yet.
    DeclaredUnverified,
    /// Matrix declares no row for `(runtime, mode)` — surge refuses to start
    /// a run against this combination.
    Unsupported,
}

impl DoctorReport {
    /// Construct an empty report. Convenience for callers that build entries
    /// incrementally.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` when no entry has a `BelowMinimum` or `ProbeFailed` status and
    /// every matrix cell is `Verified`. Useful for CI smoke tests.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.entries.iter().all(|e| {
            matches!(
                e.version_status,
                VersionStatus::Ok | VersionStatus::NotApplicable,
            ) && e.matrix.iter().all(|c| c.status == MatrixCellStatus::Verified)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(mode: SandboxMode, status: MatrixCellStatus) -> MatrixCell {
        MatrixCell {
            mode,
            status,
            flags: Vec::new(),
            note: String::new(),
        }
    }

    #[test]
    fn empty_report_is_clean() {
        let r = DoctorReport::new();
        assert!(r.is_clean());
        assert!(r.entries.is_empty());
    }

    #[test]
    fn report_is_not_clean_with_below_minimum_entry() {
        let r = DoctorReport {
            entries: vec![DoctorEntry {
                agent_name: "claude-acp".into(),
                runtime: Some(RuntimeKind::ClaudeCode),
                binary_path: Some("/usr/local/bin/claude".into()),
                detected_version: Some("1.9.0".into()),
                policy: None,
                version_status: VersionStatus::BelowMinimum,
                matrix: vec![cell(SandboxMode::WorkspaceWrite, MatrixCellStatus::Verified)],
            }],
        };
        assert!(!r.is_clean());
    }

    #[test]
    fn report_is_not_clean_with_unverified_matrix_cell() {
        let r = DoctorReport {
            entries: vec![DoctorEntry {
                agent_name: "cursor".into(),
                runtime: Some(RuntimeKind::CursorCli),
                binary_path: None,
                detected_version: None,
                policy: None,
                version_status: VersionStatus::NotApplicable,
                matrix: vec![cell(
                    SandboxMode::WorkspaceWrite,
                    MatrixCellStatus::DeclaredUnverified,
                )],
            }],
        };
        assert!(!r.is_clean());
    }

    #[test]
    fn serde_round_trip_via_json() {
        let r = DoctorReport {
            entries: vec![DoctorEntry {
                agent_name: "claude-acp".into(),
                runtime: Some(RuntimeKind::ClaudeCode),
                binary_path: Some("/usr/local/bin/claude".into()),
                detected_version: Some("2.0.1".into()),
                policy: None,
                version_status: VersionStatus::Ok,
                matrix: vec![MatrixCell {
                    mode: SandboxMode::WorkspaceWrite,
                    status: MatrixCellStatus::Verified,
                    flags: vec!["--allow-tool=Read".into()],
                    note: String::new(),
                }],
            }],
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: DoctorReport = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn matrix_cell_status_serializes_snake_case() {
        let v = serde_json::to_value(MatrixCellStatus::DeclaredUnverified).unwrap();
        assert_eq!(v, serde_json::Value::String("declared_unverified".into()));
    }

    #[test]
    fn version_status_serializes_snake_case() {
        let v = serde_json::to_value(VersionStatus::BelowMinimum).unwrap();
        assert_eq!(v, serde_json::Value::String("below_minimum".into()));
    }
}
