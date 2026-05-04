//! Engine-level and run-level configuration knobs.

use std::time::Duration;

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
}

impl Default for EngineRunConfig {
    fn default() -> Self {
        Self {
            human_input_timeout: Duration::from_secs(300),
            stage_timeout_override: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
