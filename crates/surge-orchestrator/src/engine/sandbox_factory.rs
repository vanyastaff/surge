//! Maps `SandboxConfig` → `Box<dyn Sandbox>`.
//!
//! M5 placeholder: every variant returns `AlwaysAllowSandbox`. M4 replaces
//! the match arms with real impls; the engine API doesn't change.

use surge_acp::bridge::sandbox::{AlwaysAllowSandbox, Sandbox};
use surge_core::sandbox::SandboxConfig;

/// Build a sandbox for an agent stage. `cfg = None` is the same as default
/// `SandboxMode::WorkspaceWrite` (which in M5 still maps to AlwaysAllow).
#[must_use]
pub fn build_sandbox(cfg: Option<&SandboxConfig>) -> Box<dyn Sandbox> {
    let _ = cfg; // M5: ignored. M4 will dispatch on cfg.mode.
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
            let cfg = SandboxConfig { mode, ..Default::default() };
            let _ = build_sandbox(Some(&cfg));
        }
    }
}
