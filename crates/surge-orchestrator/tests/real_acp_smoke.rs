//! Real-agent smoke harness — env contract scaffold (full driver TBD).
//!
//! ## Current scope (this commit)
//!
//! This file is the **harness skeleton** for a future real-ACP smoke test.
//! Today it verifies only the gating contract:
//!
//! - When `SURGE_REAL_ACP_BIN` and `SURGE_REAL_ACP_PROFILE` are absent /
//!   empty / point to a non-existent path, the test prints a skip banner
//!   and exits with success — keeping the deterministic CI green path
//!   covered by the mock-ACP archetype suite (`archetypes_mock_test.rs`)
//!   without requiring a real agent install.
//! - When both env vars resolve to a real binary path, the test prints a
//!   banner indicating the harness is ready and exits successfully.
//!
//! ## Out of scope (follow-up)
//!
//! Driving the engine with a real ACP child and asserting the observable
//! contract (`RunCompleted` plus ≥ 1 `TokensConsumed` event) requires
//! launching the daemon, wiring a `DaemonEngineFacade`, registering the
//! agent profile against the resolver, and a per-binary message script.
//! That work is tracked as a follow-up to the Graph engine GA milestone
//! and intentionally NOT done here — committing a half-implemented driver
//! would either silently pass on broken setups or hard-fail when the env
//! vars happen to be set in dev environments.
//!
//! When the driver lands, the env-contract assertions stay (so misconfigured
//! invocations still skip cleanly) and the post-banner section will perform
//! the actual run + event-log assertions.
//!
//! ## Enabling the harness check locally
//!
//! ```text
//! SURGE_REAL_ACP_BIN=/path/to/claude-code \
//! SURGE_REAL_ACP_PROFILE=implementer@1.0 \
//!   cargo test -p surge-orchestrator --test real_acp_smoke -- --nocapture
//! ```
//!
//! The binary path must exist on disk; the profile name will be passed
//! through to the future driver via `SURGE_REAL_ACP_PROFILE` so the same
//! harness can target Claude Code, Codex CLI, or any conformant binary
//! without recompiling.

use std::env;
use std::path::Path;

const ENV_BIN: &str = "SURGE_REAL_ACP_BIN";
const ENV_PROFILE: &str = "SURGE_REAL_ACP_PROFILE";

fn skip_banner(reason: &str) {
    eprintln!(
        "[real_acp_smoke] SKIPPED: {reason}\n\
         Set {ENV_BIN} and {ENV_PROFILE} to run this harness."
    );
}

/// Env-contract harness: confirms `SURGE_REAL_ACP_BIN` resolves to an
/// existing binary path and `SURGE_REAL_ACP_PROFILE` is non-empty when
/// the user opts in. The full real-ACP driver (run through engine,
/// assert `RunCompleted` + ≥ 1 `TokensConsumed`) is deliberately not
/// implemented here yet — see the module-level docs.
///
/// Renamed from `flow_minimal_agent_against_real_agent` to make the
/// scope honest after PR #48 review noted the previous name implied
/// coverage that is not present.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_acp_env_contract_harness() {
    let bin = match env::var(ENV_BIN) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            skip_banner(&format!("{ENV_BIN} is not set"));
            return;
        },
    };
    let profile = match env::var(ENV_PROFILE) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            skip_banner(&format!("{ENV_PROFILE} is not set"));
            return;
        },
    };

    let bin_path = Path::new(&bin);
    assert!(
        bin_path.exists(),
        "{ENV_BIN}={bin} must point to an existing binary; saw missing path"
    );
    assert!(
        bin_path.is_file(),
        "{ENV_BIN}={bin} must point to a file (not a directory)"
    );

    eprintln!(
        "[real_acp_smoke] env-contract OK: bin={bin} profile={profile}\n\
         [real_acp_smoke] full driver (RunCompleted + TokensConsumed assertions) is a Graph-engine-GA follow-up"
    );
}
