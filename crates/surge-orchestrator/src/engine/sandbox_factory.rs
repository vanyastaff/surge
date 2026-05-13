//! Maps `SandboxConfig` → `Box<dyn Sandbox>` (per-tool runtime gating) and
//! provides the launch-flag resolver hand-off (composed at session-open time).
//!
//! Two concerns live here on purpose:
//! - The `Sandbox` trait (in `surge-acp::bridge::sandbox`) governs per-tool
//!   decisions at runtime. Until the deny-list adapters land, this returns
//!   `AlwaysAllowSandbox` — tightening it is a follow-up enforcement task.
//! - The launch-flag tail comes from `surge-acp::bridge::sandbox_resolver`,
//!   which reads the bundled matrix in `surge-core`. The engine calls it at
//!   session-open time; this module only re-exports the resolver entry
//!   points for clarity.

use surge_acp::bridge::sandbox::{AlwaysAllowSandbox, Sandbox};
pub use surge_acp::bridge::sandbox_resolver::{
    ResolveContext, SandboxResolveError, resolve_launch_flags,
};
use surge_core::sandbox::SandboxConfig;

/// Build a per-tool sandbox for an agent stage.
///
/// `cfg = None` is treated as default `SandboxMode::WorkspaceWrite`.
///
/// Note: per-tool enforcement is intentionally permissive at this layer —
/// the launch-flag tail produced by `resolve_launch_flags` is what carries
/// the actual sandbox to the agent runtime. Tightening this surface to a
/// deny-list adapter is a follow-up enforcement task.
#[must_use]
pub fn build_sandbox(cfg: Option<&SandboxConfig>) -> Box<dyn Sandbox> {
    let _ = cfg;
    Box::new(AlwaysAllowSandbox)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::sandbox::{SandboxConfig, SandboxMode};

    #[test]
    fn returns_sandbox_for_none() {
        let _s = build_sandbox(None);
    }

    #[test]
    fn returns_sandbox_for_workspace_write() {
        let cfg = SandboxConfig {
            mode: SandboxMode::WorkspaceWrite,
            ..Default::default()
        };
        let _s = build_sandbox(Some(&cfg));
    }

    #[test]
    fn returns_sandbox_for_every_mode() {
        for mode in [
            SandboxMode::ReadOnly,
            SandboxMode::WorkspaceWrite,
            SandboxMode::WorkspaceNetwork,
            SandboxMode::FullAccess,
            SandboxMode::Custom,
        ] {
            let cfg = SandboxConfig {
                mode,
                ..Default::default()
            };
            let _ = build_sandbox(Some(&cfg));
        }
    }

    #[test]
    fn resolver_is_reachable_through_facade() {
        // Smoke test that the re-exported resolver entrypoints work — the
        // engine wires them at session-open time elsewhere; this exercise
        // ensures the surface stays public.
        let matrix = surge_core::default_matrix();
        let cfg = SandboxConfig {
            mode: SandboxMode::WorkspaceWrite,
            ..Default::default()
        };
        let res = resolve_launch_flags(
            surge_core::RuntimeKind::ClaudeCode,
            &cfg,
            &matrix,
            ResolveContext::Run,
        );
        assert!(res.is_ok(), "ClaudeCode + WorkspaceWrite resolves: {res:?}");
    }
}
