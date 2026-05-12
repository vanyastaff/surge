//! Launch-flag composition for a `(RuntimeKind, SandboxMode)` pair.
//!
//! This module answers one question: *given the runtime and the requested
//! sandbox mode, which command-line flags should surge append to the agent's
//! launch command?* The answer is read from
//! [`surge_core::sandbox_matrix::RuntimeSandboxMatrix`] — surge keeps the
//! mapping as data, not code, so adding a new runtime is a matrix row plus a
//! [`RuntimeKind`] variant, not a new code path.
//!
//! The complementary [`super::sandbox::Sandbox`] trait answers a different
//! question — *at runtime, may this specific tool fire?* — and is not touched
//! by this module.

use surge_core::runtime::RuntimeKind;
use surge_core::sandbox::{SandboxConfig, SandboxMode, SandboxValidationError, validate_custom};
use surge_core::sandbox_matrix::RuntimeSandboxMatrix;
use thiserror::Error;

/// Why the matrix resolver is being called.
///
/// `Run` is the production path — declared-unverified rows refuse to launch.
/// `Doctor` is the `surge doctor` smoke path — declared-unverified rows are
/// reported but allowed through so operators can probe upstream support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveContext {
    /// Normal run start. Unverified rows refuse to launch.
    Run,
    /// `surge doctor`. Unverified rows are surfaced with their declared
    /// flags (which may be empty) instead of refusing.
    Doctor,
}

/// Reason `resolve_launch_flags` declined to produce a flag list.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SandboxResolveError {
    /// The matrix has no row for `(runtime, mode)` — surge will not start a
    /// run against this combination, and `surge doctor` reports it as
    /// unsupported.
    #[error("sandbox combination {runtime:?} × {mode:?} is unsupported (no matrix row)")]
    UnsupportedCombo {
        /// Runtime that was requested.
        runtime: RuntimeKind,
        /// Sandbox mode that was requested.
        mode: SandboxMode,
    },
    /// The matrix declares the row but `verified = false` — surge has not
    /// tested it against the live runtime. Production runs refuse; `surge
    /// doctor` lets the row through so operators can investigate.
    #[error("runtime {runtime:?} is declared-unverified for mode {mode:?}; only `surge doctor` may use it")]
    UnverifiedRuntime {
        /// Runtime that was requested.
        runtime: RuntimeKind,
        /// Sandbox mode that was requested.
        mode: SandboxMode,
    },
    /// `SandboxConfig::Custom` failed pre-launch validation — surfaced
    /// per-violation so the operator sees every problem at once.
    #[error("custom sandbox config validation failed ({violations} violations)")]
    CustomInvalid {
        /// Number of accumulated violations.
        violations: usize,
        /// First violation for quick triage; the full list is available via
        /// [`SandboxResolveError::custom_violations`].
        first: SandboxValidationError,
        /// All violations, in declaration order.
        all: Vec<SandboxValidationError>,
    },
}

impl SandboxResolveError {
    /// Returns every accumulated custom-config violation, or an empty slice
    /// for non-Custom errors.
    #[must_use]
    pub fn custom_violations(&self) -> &[SandboxValidationError] {
        if let Self::CustomInvalid { all, .. } = self {
            all
        } else {
            &[]
        }
    }
}

/// Resolve the agent launch-flag tail for a `(runtime, mode)` pair against
/// the supplied matrix.
///
/// Returns the row's `flags` verbatim on success. `SandboxMode::Custom`
/// short-circuits matrix lookup — Custom mode is owned by the caller's
/// `SandboxConfig` validation path (see [`validate_custom`]); the resolver
/// only checks the structural invariants and returns an empty flag list
/// (the agent runtime composes its own argv from the validated config).
///
/// # Errors
///
/// Returns:
/// - [`SandboxResolveError::UnsupportedCombo`] when the matrix has no row.
/// - [`SandboxResolveError::UnverifiedRuntime`] when the row exists but
///   `verified = false` and `ctx = Run`.
/// - [`SandboxResolveError::CustomInvalid`] when `mode = Custom` and the
///   config fails [`validate_custom`].
pub fn resolve_launch_flags(
    runtime: RuntimeKind,
    cfg: &SandboxConfig,
    matrix: &RuntimeSandboxMatrix,
    ctx: ResolveContext,
) -> Result<Vec<String>, SandboxResolveError> {
    let mode = cfg.mode;

    if mode == SandboxMode::Custom {
        let violations = validate_custom(cfg);
        if !violations.is_empty() {
            tracing::error!(
                target: "surge_acp.sandbox",
                runtime = ?runtime,
                violations = violations.len(),
                "custom sandbox config rejected at resolve time"
            );
            return Err(SandboxResolveError::CustomInvalid {
                violations: violations.len(),
                first: violations[0].clone(),
                all: violations,
            });
        }
        tracing::debug!(
            target: "surge_acp.sandbox",
            runtime = ?runtime,
            "custom sandbox accepted; caller composes its own flags"
        );
        return Ok(Vec::new());
    }

    let Some(row) = matrix.lookup(runtime, mode) else {
        tracing::error!(
            target: "surge_acp.sandbox",
            runtime = ?runtime,
            mode = ?mode,
            "no matrix row declared for runtime/mode pair"
        );
        return Err(SandboxResolveError::UnsupportedCombo { runtime, mode });
    };

    if !row.verified && ctx == ResolveContext::Run {
        tracing::warn!(
            target: "surge_acp.sandbox",
            runtime = ?runtime,
            mode = ?mode,
            "matrix row unverified; refusing to start non-doctor run"
        );
        return Err(SandboxResolveError::UnverifiedRuntime { runtime, mode });
    }

    if matches!(mode, SandboxMode::FullAccess) {
        tracing::warn!(
            target: "surge_acp.sandbox",
            runtime = ?runtime,
            "emitting full-access flags — agent runtime owns enforcement",
        );
    }

    tracing::debug!(
        target: "surge_acp.sandbox",
        runtime = ?runtime,
        mode = ?mode,
        verified = row.verified,
        flag_count = row.flags.len(),
        "resolved sandbox launch flags",
    );
    Ok(row.flags.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use surge_core::sandbox_matrix::RuntimeSandboxRow;

    fn matrix_with(rows: Vec<RuntimeSandboxRow>) -> RuntimeSandboxMatrix {
        RuntimeSandboxMatrix::from_rows(rows)
    }

    fn verified_row(
        runtime: RuntimeKind,
        mode: SandboxMode,
        flags: &[&str],
    ) -> RuntimeSandboxRow {
        RuntimeSandboxRow::new(runtime, mode)
            .with_flags(flags.iter().copied())
            .verified()
    }

    fn unverified_row(runtime: RuntimeKind, mode: SandboxMode) -> RuntimeSandboxRow {
        RuntimeSandboxRow::new(runtime, mode).with_note("pending")
    }

    fn cfg(mode: SandboxMode) -> SandboxConfig {
        SandboxConfig {
            mode,
            ..Default::default()
        }
    }

    #[test]
    fn verified_row_returns_flags() {
        let m = matrix_with(vec![verified_row(
            RuntimeKind::ClaudeCode,
            SandboxMode::WorkspaceWrite,
            &["--allow-tool=Read", "--deny-tool=Bash"],
        )]);
        let flags = resolve_launch_flags(
            RuntimeKind::ClaudeCode,
            &cfg(SandboxMode::WorkspaceWrite),
            &m,
            ResolveContext::Run,
        )
        .expect("verified row should resolve");
        assert_eq!(flags, vec!["--allow-tool=Read", "--deny-tool=Bash"]);
    }

    #[test]
    fn missing_row_is_unsupported_combo() {
        let m = matrix_with(vec![]);
        let err = resolve_launch_flags(
            RuntimeKind::ClaudeCode,
            &cfg(SandboxMode::FullAccess),
            &m,
            ResolveContext::Run,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SandboxResolveError::UnsupportedCombo {
                runtime: RuntimeKind::ClaudeCode,
                mode: SandboxMode::FullAccess,
            }
        ));
    }

    #[test]
    fn unverified_row_refuses_in_run_context() {
        let m = matrix_with(vec![unverified_row(
            RuntimeKind::CursorCli,
            SandboxMode::WorkspaceWrite,
        )]);
        let err = resolve_launch_flags(
            RuntimeKind::CursorCli,
            &cfg(SandboxMode::WorkspaceWrite),
            &m,
            ResolveContext::Run,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SandboxResolveError::UnverifiedRuntime { .. }
        ));
    }

    #[test]
    fn unverified_row_accepted_in_doctor_context() {
        let m = matrix_with(vec![unverified_row(
            RuntimeKind::CursorCli,
            SandboxMode::WorkspaceWrite,
        )]);
        let flags = resolve_launch_flags(
            RuntimeKind::CursorCli,
            &cfg(SandboxMode::WorkspaceWrite),
            &m,
            ResolveContext::Doctor,
        )
        .expect("doctor context should accept unverified rows");
        assert!(flags.is_empty(), "unverified row carries empty flags");
    }

    #[test]
    fn custom_mode_short_circuits_matrix_lookup() {
        // Matrix has no Custom rows (and tests in surge-core enforce that).
        // Custom should still resolve when the config is structurally valid.
        let m = matrix_with(vec![]);
        let mut c = cfg(SandboxMode::Custom);
        c.writable_roots = vec![PathBuf::from("/tmp/work")];
        let flags = resolve_launch_flags(
            RuntimeKind::ClaudeCode,
            &c,
            &m,
            ResolveContext::Run,
        )
        .expect("valid custom should resolve");
        assert!(flags.is_empty());
    }

    #[test]
    fn custom_mode_with_empty_allowlists_returns_custom_invalid() {
        let m = matrix_with(vec![]);
        let err = resolve_launch_flags(
            RuntimeKind::ClaudeCode,
            &cfg(SandboxMode::Custom),
            &m,
            ResolveContext::Run,
        )
        .unwrap_err();
        assert!(matches!(err, SandboxResolveError::CustomInvalid { .. }));
        let violations = err.custom_violations();
        assert!(!violations.is_empty());
    }

    #[test]
    fn doctor_context_does_not_relax_unsupported_combo() {
        // Doctor relaxes `verified=false`, NOT missing rows. Unsupported
        // combos remain unsupported in both contexts so `surge doctor`
        // surfaces them as gaps rather than silently launching.
        let m = matrix_with(vec![]);
        let err = resolve_launch_flags(
            RuntimeKind::Goose,
            &cfg(SandboxMode::ReadOnly),
            &m,
            ResolveContext::Doctor,
        )
        .unwrap_err();
        assert!(matches!(err, SandboxResolveError::UnsupportedCombo { .. }));
    }
}
