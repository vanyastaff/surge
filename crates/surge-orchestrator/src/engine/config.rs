//! Engine-level and run-level configuration knobs.

use std::sync::Arc;
use std::time::Duration;
use surge_core::content_hash::ContentHash;
use surge_core::id::RunId;
use surge_core::keys::{KeyParseError, NodeKey};
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
    /// always populate this with `ProfileRegistry::load(..)`.
    ///
    /// This is the process-wide home + bundled registry. Each run derives
    /// its own from it — [`ProfileRegistry::for_run`] with the run's
    /// [`EngineRunConfig::project_layer`] — so the project lane is never
    /// shared across runs.
    pub profile_registry: Option<Arc<ProfileRegistry>>,
    /// Archetype/template registry used by graph validation to resolve
    /// `[metadata] template_origin` (`ValidationErrorKind::TemplateNotFound`,
    /// ticket 22). `None` keeps template checks permissive, exactly as
    /// before this field existed.
    pub archetype_registry: Option<Arc<crate::archetype_registry::ArchetypeRegistry>>,
    /// Capacity-aware dispatch policy every run's pre-dispatch check and
    /// post-429 park decision runs against (Task 12 M3, R37/R37.1;
    /// acceptance criterion B).
    ///
    /// `EngineConfig::default` carries
    /// `surge_core::capacity_config::CapacityConfig::default`'s
    /// conservative blind backoff (matching what `surge init` writes) with
    /// rotation disabled — the same default a fresh install gets. **This
    /// is not the field a config file's `[capacity]` section actually
    /// reaches production runs through** — the engine's production wiring
    /// (`surge-cli`'s `commands::engine`/`commands::bootstrap`,
    /// `surge-daemon`'s `main`) overrides this with
    /// `CapacityPolicy::from(&SurgeConfig::discover(..).capacity)` at
    /// startup, before constructing the `Engine`. A caller that builds an
    /// `Engine` directly (most tests) gets this default instead, which is
    /// intentional — those callers do not read a `surge.toml` at all.
    pub capacity: surge_core::capacity::CapacityPolicy,
    /// Engine-level fallback for [`EngineRunConfig::memory_store_path`]
    /// (Task 12 M4 review): consulted by [`crate::engine::engine::Engine::start_run`]
    /// and [`crate::engine::engine::Engine::resume_run`] whenever the
    /// per-run field is `None`. Exists specifically because
    /// `EngineRunConfig::memory_store_path` is deliberately never copied
    /// into the persisted `RunConfig` (see that field's own doc for why) —
    /// which means a *resumed* run has no way to recover a per-run
    /// override the original `start_run` call was given; it rebuilds
    /// `EngineRunConfig::default()` from scratch. A caller that needs a
    /// resumed run to keep writing to a non-default memory store (every
    /// integration test that exercises resume + a stage failure) sets it
    /// **here**, once, at `Engine` construction — not per-run — so both
    /// `start_run` and `resume_run` resolve the same value without needing
    /// anything to survive a trip through the event log.
    pub memory_store_path: Option<std::path::PathBuf>,
    /// Agent registry the engine resolves a profile's `runtime.agent_id`
    /// through — the unified catalog of every provider surge can launch.
    ///
    /// Production wiring (`surge-cli`, `surge-daemon`) populates this with
    /// the **merged** registry: the user's `[agents.*]` from `surge.toml`
    /// first, then the builtin entries, so a custom provider the operator
    /// declared is a first-class runtime — same launch path, same env,
    /// same settings seed as any builtin one. `None` falls back to
    /// `Registry::builtin()` (the legacy path most tests take), which keeps
    /// the builtin catalog working unchanged.
    ///
    /// Surge deliberately has no per-vendor branch anywhere: choosing a
    /// provider is choosing a registry entry.
    pub agent_registry: Option<Arc<surge_acp::Registry>>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            snapshot_policy: SnapshotPolicy::StageBoundary,
            profile_registry: None,
            archetype_registry: None,
            capacity: surge_core::capacity::CapacityPolicy::from(
                &surge_core::capacity_config::CapacityConfig::default(),
            ),
            memory_store_path: None,
            agent_registry: None,
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
    /// Optional bootstrap run whose produced planning artifacts should seed
    /// this run before the first stage executes.
    #[serde(default)]
    pub bootstrap_parent: Option<RunId>,
    /// Optional stable project context captured at run start.
    #[serde(default)]
    pub project_context: Option<ProjectContextSeed>,
    /// Optional accumulating project memory (`.surge/memory/`), captured at
    /// run start. Repo-resident, git-committable markdown notes concatenated
    /// into one seed so agents that bind `project_memory` carry cross-run
    /// knowledge. Reuses the [`ProjectContextSeed`] shape (`path` is the
    /// memory directory).
    #[serde(default)]
    pub project_memory: Option<ProjectContextSeed>,
    /// Additional first-class artifacts copied into a run before its first
    /// stage executes. Used by follow-up amendment runs to seed the appended
    /// roadmap slice without relying on mutable project files.
    #[serde(default)]
    pub seed_artifacts: Vec<RunSeedArtifact>,
    /// Bootstrap-flow knobs. Default values are tuned for the bundled
    /// bootstrap graph; non-bootstrap runs ignore the section entirely.
    #[serde(default)]
    pub bootstrap: BootstrapRunConfig,
    /// Live budget enforcement: resolved spend limits + breach policy, frozen
    /// at run start. Default is unlimited (no enforcement); the CLI/daemon
    /// populates it from the `[analytics]` budget settings in `surge.toml`.
    /// The engine evaluates it at every stage boundary against the run's
    /// folded cumulative cost.
    #[serde(default)]
    pub budget: surge_core::budget::BudgetGuard,
    /// Engine-level guard against a node's agent stage repeating an
    /// identical tool call, or running past a wall-clock budget, instead of
    /// burning the run's budget silently
    /// (`.autopilot/competitive-waves/spec.md` §15). `None` means
    /// "unset" — deliberately not a concrete `ToolCallLoopGuardConfig`
    /// defaulted value, so `crate::project_context::with_project_context_seed`
    /// can tell "the caller never set this" apart from "the caller set it
    /// to exactly the conservative default" and only fills the former from
    /// `SurgeConfig::tool_call_loop_guard`. `run_task.rs` resolves the
    /// final value with `.unwrap_or_default()` right before building
    /// `AgentStageParams` — nothing downstream of that point ever sees
    /// `None`.
    #[serde(default)]
    pub tool_call_loop_guard: Option<surge_core::loop_config::ToolCallLoopGuardConfig>,
    /// Threshold beyond which a tool's output moves to the artifact store
    /// instead of flowing to the node in full (§16). Same `None`-means-unset
    /// wiring and resolution as `tool_call_loop_guard` above.
    #[serde(default)]
    pub output_spill: Option<surge_core::spill_config::OutputSpillConfig>,
    /// Test-only override of `MemoryStore::default_path()`
    /// (`~/.surge/memory.db`), consumed by both
    /// `engine::hooks::memory_writeback::record_node_failure` (writes a
    /// failure claim) and `project_context::load_memory_claims_seed` (reads
    /// the claims pack a run seeds `project_memory` from). `None` — the
    /// only value any production caller sets — keeps the real default path;
    /// integration tests set `Some(tempdir_path)` so a run's memory reads
    /// and writes land in a throwaway store instead of mutating the
    /// process-wide `$HOME`/`SURGE_HOME` environment variables
    /// (`tests/memory_writeback_test.rs`,
    /// `project_context::with_project_context_seed_memory_claims_tests`).
    /// Deliberately never copied into `surge_core::run_event::RunConfig`
    /// (`Engine::startup_run_events`'s `core_run_config`), so it is not part
    /// of the persisted run schema and does not, by itself, survive a
    /// daemon restart + resume: `Engine::resume_run` rebuilds
    /// `EngineRunConfig::default()` from scratch (`memory_store_path:
    /// None`), with nothing in the event log to recover a per-run override
    /// from.
    ///
    /// **A resumed run is not left writing to the real default path on
    /// that account** (Task 12 M4 review, closed the same milestone that
    /// made resume routine instead of restart-only): [`EngineConfig::
    /// memory_store_path`] is the engine-level fallback both `start_run`
    /// and `resume_run` consult when this field is `None` — set it once at
    /// `Engine` construction (not per-run) and every run on that engine,
    /// including a resumed one, resolves the same store.
    #[serde(default)]
    pub memory_store_path: Option<std::path::PathBuf>,
    /// The repository's own `.surge/` layer this run resolves profiles
    /// (and, later, flows and skills) against — project → home → bundled.
    /// Resolved at run start by [`crate::engine::engine::Engine::start_run`]
    /// into a run-scoped [`crate::profile_loader::ProfileRegistry`]
    /// (`for_run`), so two repositories served by one daemon never see each
    /// other's project profiles. `None` binds no project lane: the run
    /// resolves home + bundled only.
    ///
    /// Every launcher that goes through
    /// `crate::project_context::with_project_context_seed` gets
    /// `ProjectLayer::for_project(project_root)` here. The layer only
    /// enriches a wired [`EngineConfig::profile_registry`]; with no
    /// registry the legacy mock-only path stays exactly that.
    ///
    /// Not copied into the persisted `surge_core::run_event::RunConfig`
    /// today, so a resumed run cannot recover it from the event log:
    /// [`crate::engine::engine::Engine::resume_run`] re-derives it from
    /// the worktree path, which is what the daemon IPC server and the CLI
    /// seeded from. Persisting the project root on `RunConfig` is what
    /// makes that exact for every launcher.
    #[serde(default)]
    pub project_layer: Option<surge_core::ProjectLayer>,
    /// Why this run exists, when it was dispatched for a queued project
    /// task (ADR-0020). Copied into the persisted
    /// `surge_core::run_event::RunConfig::origin`, where the daemon's
    /// reconcile sweep reads it back to settle the queue row against the
    /// run's own fate. `None` for direct runs (bootstrap, CLI, templates).
    #[serde(default)]
    pub origin: Option<surge_core::run_event::RunOrigin>,
    /// Events the caller wants durably in the run's log *before* the first
    /// stage executes — currently `ComposedArtifactInstalled`, recorded by
    /// the task scheduler when it installed a composed flow for this task
    /// (T12). A run's log is the authority for what it used; an install that
    /// happened one tick earlier would otherwise exist only outside it.
    #[serde(default)]
    pub startup_events: Vec<surge_core::run_event::VersionedEventPayload>,
}

/// Stable project context input copied into a run's artifact store.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectContextSeed {
    /// Source file path that supplied the context.
    pub path: std::path::PathBuf,
    /// Captured markdown content.
    pub content: String,
    /// Hash of `content`, computed before the run starts.
    pub hash: ContentHash,
}

impl ProjectContextSeed {
    /// Build a seed and compute its content hash.
    #[must_use]
    pub fn new(path: std::path::PathBuf, content: String) -> Self {
        let hash = ContentHash::compute(content.as_bytes());
        Self {
            path,
            content,
            hash,
        }
    }
}

/// Caller-supplied artifact copied into the run worktree at start.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RunSeedArtifact {
    /// Artifact name visible through `ArtifactSource::RunArtifact`.
    pub name: String,
    /// Portable relative path inside the run worktree.
    pub relative_path: std::path::PathBuf,
    /// Captured artifact body.
    pub content: String,
    /// Synthetic or real producer node recorded in `ArtifactProduced`.
    pub producer: NodeKey,
    /// Hash of `content`, computed before the run starts.
    pub hash: ContentHash,
}

impl RunSeedArtifact {
    /// Build a seed artifact and compute its content hash.
    ///
    /// # Errors
    /// Returns [`KeyParseError`] when `producer` is not a valid node key.
    pub fn new(
        name: impl Into<String>,
        relative_path: impl Into<std::path::PathBuf>,
        content: impl Into<String>,
        producer: &str,
    ) -> Result<Self, KeyParseError> {
        let content = content.into();
        let hash = ContentHash::compute(content.as_bytes());
        Ok(Self {
            name: name.into(),
            relative_path: relative_path.into(),
            content,
            producer: NodeKey::try_from(producer)?,
            hash,
        })
    }
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
            bootstrap_parent: None,
            project_context: None,
            project_memory: None,
            seed_artifacts: Vec::new(),
            bootstrap: BootstrapRunConfig::default(),
            budget: surge_core::budget::BudgetGuard::default(),
            tool_call_loop_guard: None,
            output_spill: None,
            memory_store_path: None,
            project_layer: None,
            origin: None,
            startup_events: Vec::new(),
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
    fn engine_run_config_default_has_no_memory_store_path_override() {
        let cfg = EngineRunConfig::default();
        assert!(cfg.memory_store_path.is_none());
    }

    #[test]
    fn engine_run_config_missing_memory_store_path_deserializes_to_none() {
        // Persisted/legacy configs from before this field existed must still
        // decode, defaulting to the real `MemoryStore::default_path()`.
        let json = r#"{"human_input_timeout":"5m","stage_timeout_override":null}"#;
        let parsed: EngineRunConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.memory_store_path.is_none());
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
            bootstrap_parent: None,
            project_context: None,
            project_memory: None,
            seed_artifacts: Vec::new(),
            bootstrap: BootstrapRunConfig::default(),
            budget: surge_core::budget::BudgetGuard::default(),
            tool_call_loop_guard: None,
            output_spill: None,
            memory_store_path: None,
            project_layer: None,
            origin: None,
            startup_events: Vec::new(),
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
        assert!(parsed.bootstrap_parent.is_none());
    }

    #[test]
    fn engine_run_config_serializes_initial_prompt() {
        let cfg = EngineRunConfig {
            human_input_timeout: Duration::from_secs(60),
            stage_timeout_override: None,
            mcp_servers: Vec::new(),
            initial_prompt: "fix the broken cart-total bug".into(),
            bootstrap_parent: None,
            project_context: None,
            project_memory: None,
            seed_artifacts: Vec::new(),
            bootstrap: BootstrapRunConfig::default(),
            budget: surge_core::budget::BudgetGuard::default(),
            tool_call_loop_guard: None,
            output_spill: None,
            memory_store_path: None,
            project_layer: None,
            origin: None,
            startup_events: Vec::new(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: EngineRunConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.initial_prompt, "fix the broken cart-total bug");
    }

    #[test]
    fn engine_run_config_serializes_bootstrap_parent() {
        let parent = RunId::new();
        let cfg = EngineRunConfig {
            bootstrap_parent: Some(parent),
            ..EngineRunConfig::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: EngineRunConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.bootstrap_parent, Some(parent));
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
            bootstrap_parent: None,
            project_context: None,
            project_memory: None,
            seed_artifacts: Vec::new(),
            bootstrap: BootstrapRunConfig { edit_loop_cap: 5 },
            budget: surge_core::budget::BudgetGuard::default(),
            tool_call_loop_guard: None,
            output_spill: None,
            memory_store_path: None,
            project_layer: None,
            origin: None,
            startup_events: Vec::new(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: EngineRunConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.bootstrap.edit_loop_cap, 5);
    }

    #[test]
    fn run_seed_artifact_computes_hash_and_validates_producer() {
        let seed = RunSeedArtifact::new(
            "roadmap_amendment",
            ".surge/roadmap_amendment.md",
            "## New work",
            "roadmap_seed",
        )
        .unwrap();
        assert_eq!(seed.name, "roadmap_amendment");
        assert_eq!(seed.producer.as_ref(), "roadmap_seed");
        assert_eq!(seed.hash, ContentHash::compute(b"## New work"));
    }
}
