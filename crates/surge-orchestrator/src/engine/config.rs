//! Engine-level and run-level configuration knobs.

use std::sync::Arc;
use std::time::Duration;
use surge_core::mcp_config::McpServerRef;

use crate::profile_loader::ProfileRegistry;

/// Top-level engine configuration, shared across all runs.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Controls when the engine persists a snapshot to storage.
    pub snapshot_policy: SnapshotPolicy,
    /// Registry used by agent stages to resolve `agent_config.profile`
    /// references into a fully merged [`surge_core::profile::Profile`]
    /// (and from there to an `AgentKind` via `runtime.agent_id`).
    ///
    /// `None` keeps the legacy M5 mock-only fast path active for tests
    /// and pre-registry callers; production wiring (CLI / daemon) should
    /// always populate this with `ProfileRegistry::load()`.
    pub profile_registry: Option<Arc<ProfileRegistry>>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            snapshot_policy: SnapshotPolicy::StageBoundary,
            profile_registry: None,
        }
    }
}

/// Controls when the engine writes a snapshot blob to storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotPolicy {
    /// Snapshot after every successful stage. M5 default and only variant.
    StageBoundary,
}

/// Per-run configuration; passed to `Engine::start_run` and `Engine::resume_run`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineRunConfig {
    /// Default human-input timeout if a `HumanGate` doesn't override.
    /// Default 5 minutes.
    #[serde(with = "humantime_serde")]
    pub human_input_timeout: Duration,
    /// Per-stage timeout cap. `None` = use `AgentConfig::limits.timeout_seconds`
    /// for agent stages. Reserved for M6 daemon-level overrides.
    #[serde(default, with = "humantime_serde::option")]
    pub stage_timeout_override: Option<Duration>,
    /// Per-run MCP server registry. When non-empty, [`Engine::start_run`]
    /// builds an `Arc<surge_mcp::McpRegistry>` from these entries
    /// before dispatching to the run task; agent stages then expose
    /// the configured MCP tools via `RoutingToolDispatcher`. Defaults
    /// to empty (no MCP).
    ///
    /// As of M7 there is no user-facing CLI config loader for this
    /// field — programmatic callers populate it directly. A
    /// `~/.surge/config.toml` loader and `--mcp-config <path>` CLI
    /// flag are planned for M8+ scope.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerRef>,
    /// Free-form prompt that initiated the run. Surfaced to bootstrap profiles
    /// (and any other agent stage) via `ArtifactSource::InitialPrompt`, which
    /// resolves through the standard binding path against a synthesised
    /// `RunMemory.artifacts["user_prompt"]` entry seeded by `Engine::start_run`.
    /// Empty string disables seeding (the legacy default for non-bootstrap
    /// runs).
    #[serde(default)]
    pub initial_prompt: String,
    /// Bootstrap-flow knobs. Default values are tuned for the bundled
    /// bootstrap graph; non-bootstrap runs ignore the section entirely.
    #[serde(default)]
    pub bootstrap: BootstrapRunConfig,
}

/// Knobs for the bootstrap-driven adaptive flow. See
/// `EngineRunConfig::bootstrap`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BootstrapRunConfig {
    /// Maximum number of `BootstrapEditRequested` cycles per stage before the
    /// engine bails out with `StageError::EditLoopCapExceeded`. Default `3`
    /// (matches Decision 4 / ADR 0004 in the milestone plan). Set to `0` to
    /// disable the cap (not recommended in production — used by integration
    /// tests that need to exercise unbounded loops).
    #[serde(default = "default_edit_loop_cap")]
    pub edit_loop_cap: u32,
}

fn default_edit_loop_cap() -> u32 {
    3
}

impl Default for BootstrapRunConfig {
    fn default() -> Self {
        Self {
            edit_loop_cap: default_edit_loop_cap(),
        }
    }
}

impl Default for EngineRunConfig {
    fn default() -> Self {
        Self {
            human_input_timeout: Duration::from_secs(300),
            stage_timeout_override: None,
            mcp_servers: Vec::new(),
            initial_prompt: String::new(),
            bootstrap: BootstrapRunConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use surge_core::mcp_config::McpTransportConfig;

    #[test]
    fn engine_config_default_uses_stage_boundary() {
        let c = EngineConfig::default();
        assert_eq!(c.snapshot_policy, SnapshotPolicy::StageBoundary);
    }

    #[test]
    fn engine_config_default_has_no_profile_registry() {
        let c = EngineConfig::default();
        assert!(c.profile_registry.is_none());
    }

    #[test]
    fn run_config_default_human_input_is_5_minutes() {
        let c = EngineRunConfig::default();
        assert_eq!(c.human_input_timeout, Duration::from_secs(300));
    }

    #[test]
    fn engine_run_config_default_mcp_servers_empty() {
        let cfg = EngineRunConfig::default();
        assert!(cfg.mcp_servers.is_empty());
    }

    #[test]
    fn engine_run_config_with_mcp_servers_serde_roundtrip() {
        let cfg = EngineRunConfig {
            human_input_timeout: Duration::from_secs(120),
            stage_timeout_override: None,
            mcp_servers: vec![McpServerRef::new(
                "playwright".into(),
                McpTransportConfig::stdio(PathBuf::from("mcp-playwright"), vec![], HashMap::new()),
                None,
                Duration::from_secs(60),
                true,
            )],
            initial_prompt: String::new(),
            bootstrap: BootstrapRunConfig::default(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: EngineRunConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.mcp_servers.len(), 1);
        assert_eq!(parsed.mcp_servers[0].name, "playwright");
    }

    #[test]
    fn engine_run_config_missing_mcp_servers_deserializes_to_empty() {
        // Old serialised blobs without the field should still round-trip.
        let json = r#"{"human_input_timeout":"5m","stage_timeout_override":null}"#;
        let parsed: EngineRunConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.mcp_servers.is_empty());
        // Legacy blobs without `initial_prompt` must default to the empty
        // string so the engine treats them as non-bootstrap runs.
        assert!(parsed.initial_prompt.is_empty());
    }

    #[test]
    fn engine_run_config_serializes_initial_prompt() {
        let cfg = EngineRunConfig {
            human_input_timeout: Duration::from_secs(60),
            stage_timeout_override: None,
            mcp_servers: Vec::new(),
            initial_prompt: "fix the broken cart-total bug".into(),
            bootstrap: BootstrapRunConfig::default(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: EngineRunConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.initial_prompt, "fix the broken cart-total bug");
    }

    #[test]
    fn bootstrap_run_config_default_cap_is_three() {
        let cfg = EngineRunConfig::default();
        assert_eq!(cfg.bootstrap.edit_loop_cap, 3);
    }

    #[test]
    fn bootstrap_run_config_legacy_json_defaults_to_default_cap() {
        // Persisted run configs from before Task 9 do not carry a bootstrap
        // block; they must still decode and pick up the default cap.
        let json = r#"{"human_input_timeout":"5m","stage_timeout_override":null}"#;
        let parsed: EngineRunConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.bootstrap.edit_loop_cap, 3);
    }

    #[test]
    fn bootstrap_run_config_serde_roundtrip() {
        let cfg = EngineRunConfig {
            human_input_timeout: Duration::from_secs(60),
            stage_timeout_override: None,
            mcp_servers: Vec::new(),
            initial_prompt: String::new(),
            bootstrap: BootstrapRunConfig { edit_loop_cap: 5 },
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: EngineRunConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.bootstrap.edit_loop_cap, 5);
    }
}
