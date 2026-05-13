//! Sandbox-mode matrix data: which runtime supports which [`SandboxMode`] and
//! with what launch flags.
//!
//! The matrix is **data**, not code — new runtimes ship by adding a
//! [`RuntimeKind`] variant and at least one row in
//! `crates/surge-core/bundled/sandbox/matrix.toml`. The resolver in
//! `surge-acp` consumes this table; adapters never embed their own per-mode
//! flag logic.
//!
//! Two correctness invariants are enforced by tests:
//! - A row may declare empty `flags` only when `verified = false`.
//!   `verified = true` implies a concrete launch-flag mapping.
//! - The matrix never declares a [`SandboxMode::Custom`] row — `Custom` is
//!   handled out-of-band through [`crate::sandbox::SandboxConfig`] validation.

use crate::runtime::RuntimeKind;
use crate::sandbox::SandboxMode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const BUNDLED_MATRIX_TOML: &str = include_str!("../bundled/sandbox/matrix.toml");

/// A single row in the runtime × mode sandbox table.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSandboxRow {
    /// Runtime the row applies to.
    pub runtime: RuntimeKind,
    /// Sandbox mode the row maps from.
    pub mode: SandboxMode,
    /// Launch flags appended to the agent's command line for this pair.
    ///
    /// Empty when `verified = false` — declared-but-unverified rows record
    /// intent only. The resolver in `surge-acp` refuses to start a run from
    /// an unverified row unless the caller is `surge doctor`.
    #[serde(default)]
    pub flags: Vec<String>,
    /// Environment variables injected when launching the agent for this pair.
    ///
    /// Ordered via `BTreeMap` so serialized payloads (and event-log entries
    /// downstream) are deterministic — `HashMap` ordering would defeat
    /// replay-byte-equality property tests.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// `true` when surge has tested this combination against the live runtime.
    #[serde(default)]
    pub verified: bool,
    /// Minimum runtime version known to support this row.
    ///
    /// `None` means surge has not declared a minimum. `surge doctor` reads
    /// this field to flag stale binaries.
    #[serde(default)]
    pub min_version: Option<semver::VersionReq>,
    /// Human-readable rationale or upstream pointer.
    ///
    /// Required for unverified rows so contributors can trace the gap to a
    /// concrete upstream issue or docs page. Optional otherwise.
    #[serde(default)]
    pub note: String,
}

impl RuntimeSandboxRow {
    /// Constructor for tests and callers outside this crate.
    ///
    /// Struct-literal syntax does not compile across crate boundaries because
    /// the type is `#[non_exhaustive]`; this builder fills the optional
    /// fields with their defaults and lets the caller set the required ones
    /// inline.
    #[must_use]
    pub fn new(runtime: RuntimeKind, mode: SandboxMode) -> Self {
        Self {
            runtime,
            mode,
            flags: Vec::new(),
            env: BTreeMap::new(),
            verified: false,
            min_version: None,
            note: String::new(),
        }
    }

    /// Builder: replace `flags`.
    #[must_use]
    pub fn with_flags<I, S>(mut self, flags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.flags = flags.into_iter().map(Into::into).collect();
        self
    }

    /// Builder: mark as verified.
    #[must_use]
    pub fn verified(mut self) -> Self {
        self.verified = true;
        self
    }

    /// Builder: attach an explanatory note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }
}

/// In-memory representation of the runtime × mode sandbox table.
///
/// Construct via [`default_matrix`] (loads the bundled TOML) or
/// [`RuntimeSandboxMatrix::from_rows`] in tests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSandboxMatrix {
    rows: Vec<RuntimeSandboxRow>,
}

impl RuntimeSandboxMatrix {
    /// Empty matrix — useful for tests that build rows up incrementally.
    #[must_use]
    pub fn empty() -> Self {
        Self { rows: Vec::new() }
    }

    /// Construct from a list of rows. Declaration order is preserved.
    #[must_use]
    pub fn from_rows(rows: Vec<RuntimeSandboxRow>) -> Self {
        Self { rows }
    }

    /// Look up a row by exact `(runtime, mode)`. Returns `None` when no row
    /// is declared for the pair (i.e. the combo is unsupported).
    #[must_use]
    pub fn lookup(&self, runtime: RuntimeKind, mode: SandboxMode) -> Option<&RuntimeSandboxRow> {
        self.rows
            .iter()
            .find(|r| r.runtime == runtime && r.mode == mode)
    }

    /// `true` when the matrix declares no row for `(runtime, mode)`.
    ///
    /// A *declared-but-unverified* row is **not** unsupported — it is declared
    /// and the resolver routes it to a distinct error so `surge doctor` can
    /// still exercise it.
    #[must_use]
    pub fn unsupported(&self, runtime: RuntimeKind, mode: SandboxMode) -> bool {
        self.lookup(runtime, mode).is_none()
    }

    /// Iterator over rows where `verified == true`.
    pub fn verified_only(&self) -> impl Iterator<Item = &RuntimeSandboxRow> + '_ {
        self.rows.iter().filter(|r| r.verified)
    }

    /// All rows in declaration order.
    #[must_use]
    pub fn rows(&self) -> &[RuntimeSandboxRow] {
        &self.rows
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct BundledMatrixDocument {
    rows: Vec<RuntimeSandboxRow>,
}

/// Default sandbox matrix bundled into the surge binary at compile time.
///
/// # Panics
///
/// Panics at startup if `bundled/sandbox/matrix.toml` fails to parse. This is
/// a build-time invariant — the file ships with the crate and must be valid.
#[must_use]
pub fn default_matrix() -> RuntimeSandboxMatrix {
    let doc: BundledMatrixDocument = toml::from_str(BUNDLED_MATRIX_TOML)
        .expect("bundled sandbox matrix.toml must parse — repo invariant");
    RuntimeSandboxMatrix { rows: doc.rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_matrix_parses() {
        let m = default_matrix();
        assert!(!m.rows().is_empty(), "bundled matrix must declare rows");
    }

    #[test]
    fn no_row_is_verified_with_empty_flags() {
        let m = default_matrix();
        for row in m.rows() {
            assert!(
                !(row.verified && row.flags.is_empty()),
                "row {:?}/{:?} is verified but has no flags",
                row.runtime,
                row.mode,
            );
        }
    }

    #[test]
    fn matrix_never_declares_custom_rows() {
        let m = default_matrix();
        for row in m.rows() {
            assert_ne!(
                row.mode,
                SandboxMode::Custom,
                "matrix must not declare a Custom row (SandboxConfig::Custom handles that path); offender: {:?}",
                row.runtime,
            );
        }
    }

    #[test]
    fn lookup_returns_some_for_declared_pair() {
        let m = default_matrix();
        let row = m.lookup(RuntimeKind::ClaudeCode, SandboxMode::WorkspaceWrite);
        assert!(
            row.is_some(),
            "default matrix must declare ClaudeCode + WorkspaceWrite",
        );
    }

    #[test]
    fn unsupported_returns_true_for_custom() {
        // SandboxMode::Custom is not part of the runtime × mode matrix.
        let m = default_matrix();
        assert!(m.unsupported(RuntimeKind::ClaudeCode, SandboxMode::Custom));
    }

    #[test]
    fn unverified_rows_carry_non_empty_note() {
        // Editorial rule: declared-but-unverified rows must explain the gap so
        // contributors can find the upstream issue or docs page.
        let m = default_matrix();
        for row in m.rows() {
            if !row.verified {
                assert!(
                    !row.note.is_empty(),
                    "unverified row {:?}/{:?} must carry a non-empty note",
                    row.runtime,
                    row.mode,
                );
            }
        }
    }

    #[test]
    fn round_trip_via_toml() {
        let original = default_matrix();
        let doc = BundledMatrixDocument {
            rows: original.rows().to_vec(),
        };
        let serialized = toml::to_string(&doc).expect("serialize matrix");
        let parsed: BundledMatrixDocument =
            toml::from_str(&serialized).expect("re-parse matrix");
        assert_eq!(original.rows(), parsed.rows.as_slice());
    }

    #[test]
    fn from_rows_preserves_order() {
        let rows = vec![
            RuntimeSandboxRow {
                runtime: RuntimeKind::ClaudeCode,
                mode: SandboxMode::ReadOnly,
                flags: vec!["--a".into()],
                env: BTreeMap::new(),
                verified: true,
                min_version: None,
                note: String::new(),
            },
            RuntimeSandboxRow {
                runtime: RuntimeKind::Codex,
                mode: SandboxMode::WorkspaceWrite,
                flags: vec!["--b".into()],
                env: BTreeMap::new(),
                verified: true,
                min_version: None,
                note: String::new(),
            },
        ];
        let m = RuntimeSandboxMatrix::from_rows(rows.clone());
        assert_eq!(m.rows(), rows.as_slice());
    }

    // ── Property tests: resolver totality ────────────────────────────────────
    //
    // The default matrix must be **total** over every `(RuntimeKind, SandboxMode)`
    // pair the system knows about. "Total" here means three rules:
    //   1. `lookup` never panics.
    //   2. `unsupported` is the logical negation of `lookup`.
    //   3. No row claims `verified = true` while shipping empty `flags`.
    //
    // Encoded as proptest so any future variant additions (new RuntimeKind /
    // SandboxMode) automatically widen the property space.

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 256,
            ..Default::default()
        })]

        #[test]
        fn lookup_is_total_for_every_pair(
            runtime in proptest::sample::select(&[
                RuntimeKind::ClaudeCode,
                RuntimeKind::Codex,
                RuntimeKind::Gemini,
                RuntimeKind::CursorCli,
                RuntimeKind::CopilotCli,
                RuntimeKind::OpenCode,
                RuntimeKind::Goose,
            ][..]),
            mode in proptest::sample::select(&[
                SandboxMode::ReadOnly,
                SandboxMode::WorkspaceWrite,
                SandboxMode::WorkspaceNetwork,
                SandboxMode::FullAccess,
                SandboxMode::Custom,
            ][..]),
        ) {
            let matrix = default_matrix();
            let row = matrix.lookup(runtime, mode);
            // Rule 2: unsupported ↔ lookup is None.
            assert_eq!(row.is_none(), matrix.unsupported(runtime, mode));
            // Rule 3: verified rows always carry concrete flags.
            if let Some(r) = row {
                assert!(
                    !(r.verified && r.flags.is_empty()),
                    "row {:?}/{:?} is verified but empty",
                    r.runtime,
                    r.mode,
                );
            }
        }

        #[test]
        fn lookup_is_idempotent(
            runtime in proptest::sample::select(&[
                RuntimeKind::ClaudeCode,
                RuntimeKind::Codex,
                RuntimeKind::Gemini,
            ][..]),
            mode in proptest::sample::select(&[
                SandboxMode::ReadOnly,
                SandboxMode::WorkspaceWrite,
                SandboxMode::WorkspaceNetwork,
                SandboxMode::FullAccess,
            ][..]),
        ) {
            let matrix = default_matrix();
            // Two consecutive lookups produce identical results.
            assert_eq!(matrix.lookup(runtime, mode), matrix.lookup(runtime, mode));
        }
    }

    #[test]
    fn verified_only_filters() {
        let rows = vec![
            RuntimeSandboxRow {
                runtime: RuntimeKind::ClaudeCode,
                mode: SandboxMode::ReadOnly,
                flags: vec!["--x".into()],
                env: BTreeMap::new(),
                verified: true,
                min_version: None,
                note: String::new(),
            },
            RuntimeSandboxRow {
                runtime: RuntimeKind::Goose,
                mode: SandboxMode::ReadOnly,
                flags: Vec::new(),
                env: BTreeMap::new(),
                verified: false,
                min_version: None,
                note: "pending".into(),
            },
        ];
        let m = RuntimeSandboxMatrix::from_rows(rows);
        let v: Vec<_> = m.verified_only().collect();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].runtime, RuntimeKind::ClaudeCode);
    }
}
