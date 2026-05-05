//! Engine-level and run-level configuration knobs.

use std::time::Duration;
use surge_core::mcp_config::McpServerRef;

/// Top-level engine configuration, shared across all runs.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Controls when the engine persists a snapshot to storage.
    pub snapshot_policy: SnapshotPolicy,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            snapshot_policy: SnapshotPolicy::StageBoundary,
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
}

impl Default for EngineRunConfig {
    fn default() -> Self {
        Self {
            human_input_timeout: Duration::from_secs(300),
            stage_timeout_override: None,
            mcp_servers: Vec::new(),
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
    }
}
