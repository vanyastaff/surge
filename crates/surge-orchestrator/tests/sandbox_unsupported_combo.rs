//! Negative tests for the sandbox-matrix resolver.
//!
//! Asserts the bundled matrix correctly:
//! - Refuses `(Gemini, ReadOnly)` in `ResolveContext::Run` (declared-but-
//!   unverified row), surfacing `SandboxResolveError::UnverifiedRuntime`.
//! - Accepts the same combo in `ResolveContext::Doctor` so `surge doctor`
//!   can probe upstream support without launching.
//! - Refuses `(ClaudeCode, Custom)` because Custom is intentionally not
//!   declared in the matrix — Custom sandboxing flows through
//!   `SandboxConfig::Custom` validation, not matrix lookup.
//! - Refuses combinations with an empty (per-mode) flag list under Run
//!   context but accepts them under Doctor.

use surge_acp::bridge::{ResolveContext, SandboxResolveError, resolve_launch_flags};
use surge_core::default_matrix;
use surge_core::runtime::RuntimeKind;
use surge_core::sandbox::{SandboxConfig, SandboxMode};

fn cfg(mode: SandboxMode) -> SandboxConfig {
    SandboxConfig {
        mode,
        ..Default::default()
    }
}

#[test]
fn gemini_read_only_is_unverified_in_run_context() {
    let matrix = default_matrix();
    let err = resolve_launch_flags(
        RuntimeKind::Gemini,
        &cfg(SandboxMode::ReadOnly),
        &matrix,
        ResolveContext::Run,
    )
    .expect_err("Gemini + ReadOnly is declared-unverified");
    assert!(matches!(
        err,
        SandboxResolveError::UnverifiedRuntime {
            runtime: RuntimeKind::Gemini,
            mode: SandboxMode::ReadOnly,
        }
    ));
}

#[test]
fn gemini_read_only_is_accepted_in_doctor_context() {
    let matrix = default_matrix();
    let flags = resolve_launch_flags(
        RuntimeKind::Gemini,
        &cfg(SandboxMode::ReadOnly),
        &matrix,
        ResolveContext::Doctor,
    )
    .expect("doctor accepts declared-unverified rows");
    // Unverified rows carry empty flags by construction.
    assert!(flags.is_empty());
}

#[test]
fn claude_code_workspace_write_resolves_to_concrete_flags() {
    // Sanity check that the verified-and-default mode for the v0.1 verified
    // runtime resolves to non-empty flags.
    let matrix = default_matrix();
    let flags = resolve_launch_flags(
        RuntimeKind::ClaudeCode,
        &cfg(SandboxMode::WorkspaceWrite),
        &matrix,
        ResolveContext::Run,
    )
    .expect("ClaudeCode + WorkspaceWrite is verified");
    assert!(!flags.is_empty(), "verified rows must produce real flags");
}

#[test]
fn unverified_cursor_workspace_write_refuses_in_run_context() {
    // Cursor is declared-unverified across all modes in the v0.1 matrix.
    let matrix = default_matrix();
    let err = resolve_launch_flags(
        RuntimeKind::CursorCli,
        &cfg(SandboxMode::WorkspaceWrite),
        &matrix,
        ResolveContext::Run,
    )
    .expect_err("Cursor declared-unverified must refuse non-doctor runs");
    assert!(matches!(
        err,
        SandboxResolveError::UnverifiedRuntime {
            runtime: RuntimeKind::CursorCli,
            ..
        }
    ));
}

#[test]
fn custom_mode_with_no_allowlists_is_invalid() {
    // Custom mode never has a matrix row. The resolver delegates to
    // SandboxConfig::Custom validation; an all-empty config is rejected as
    // CustomInvalid before any matrix lookup.
    let matrix = default_matrix();
    let err = resolve_launch_flags(
        RuntimeKind::ClaudeCode,
        &cfg(SandboxMode::Custom),
        &matrix,
        ResolveContext::Run,
    )
    .expect_err("empty Custom config is invalid");
    assert!(matches!(err, SandboxResolveError::CustomInvalid { .. }));
}
