# Phase 0: Foundation — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Complete Phase 0 Foundation — add missing core types (spec, events), extend config, wire CLI commands to ACP infrastructure, add init command.

**Architecture:** surge-core gets new modules (spec.rs, event.rs) with domain types. SurgeEvent moves from surge-acp to surge-core for proper layering. CLI commands (ping, prompt, agent test) use existing AgentPool. New `init` command generates surge.toml.

**Tech Stack:** Rust 2024, serde/toml, tokio, agent-client-protocol 0.6, clap 4, thiserror 2

---

## Status Check

Already complete:
- surge-core: id.rs, state.rs, error.rs, config.rs (with full tests)
- surge-acp: client.rs (ACP Client trait), connection.rs (AgentConnection), pool.rs (AgentPool)
- surge-cli: agent list, config show commands

Remaining work (this plan):
- Task 0.1: spec.rs, event.rs, extend SurgeConfig
- Task 0.2: Add mock-based tests for ACP Client
- Task 0.4: Wire ping/prompt/test to ACP, add init command

Task 0.3 (AgentPool) is already complete — skipped.

---

### Task 1: Add `spec.rs` to surge-core

**Files:**
- Create: `crates/surge-core/src/spec.rs`
- Modify: `crates/surge-core/src/lib.rs`

**Step 1: Write the failing test**

Add at the bottom of the new file `crates/surge-core/src/spec.rs`:

```rust
//! Spec types for Surge task definitions.

use serde::{Deserialize, Serialize};

use crate::id::{SpecId, SubtaskId};

/// Complexity level for a task or subtask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Complexity {
    Simple,
    Standard,
    Complex,
}

/// Acceptance criteria for a subtask.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceCriteria {
    /// Human-readable description of what must be true.
    pub description: String,
    /// Whether this criterion is currently met.
    #[serde(default)]
    pub met: bool,
}

/// A subtask within a spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subtask {
    /// Unique identifier.
    pub id: SubtaskId,
    /// Short title.
    pub title: String,
    /// Detailed description of work to do.
    pub description: String,
    /// Estimated complexity.
    pub complexity: Complexity,
    /// Files this subtask will touch.
    #[serde(default)]
    pub files: Vec<String>,
    /// Acceptance criteria that must pass.
    #[serde(default)]
    pub acceptance_criteria: Vec<AcceptanceCriteria>,
    /// Dependencies on other subtask IDs (must complete first).
    #[serde(default)]
    pub depends_on: Vec<SubtaskId>,
}

/// A complete spec describing a unit of work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    /// Unique identifier.
    pub id: SpecId,
    /// Short title.
    pub title: String,
    /// Detailed description.
    pub description: String,
    /// Overall complexity.
    pub complexity: Complexity,
    /// Ordered list of subtasks.
    #[serde(default)]
    pub subtasks: Vec<Subtask>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_complexity_roundtrip() {
        let values = [Complexity::Simple, Complexity::Standard, Complexity::Complex];
        for val in &values {
            let serialized = toml::to_string(val).unwrap();
            let deserialized: Complexity = toml::from_str(&serialized).unwrap();
            assert_eq!(*val, deserialized);
        }
    }

    #[test]
    fn test_spec_toml_roundtrip() {
        let spec = Spec {
            id: SpecId::new(),
            title: "Test spec".to_string(),
            description: "A test specification".to_string(),
            complexity: Complexity::Standard,
            subtasks: vec![
                Subtask {
                    id: SubtaskId::new(),
                    title: "First subtask".to_string(),
                    description: "Do the first thing".to_string(),
                    complexity: Complexity::Simple,
                    files: vec!["src/main.rs".to_string()],
                    acceptance_criteria: vec![
                        AcceptanceCriteria {
                            description: "Code compiles".to_string(),
                            met: false,
                        },
                    ],
                    depends_on: vec![],
                },
            ],
        };

        let toml_str = toml::to_string(&spec).unwrap();
        let deserialized: Spec = toml::from_str(&toml_str).unwrap();

        assert_eq!(deserialized.title, "Test spec");
        assert_eq!(deserialized.complexity, Complexity::Standard);
        assert_eq!(deserialized.subtasks.len(), 1);
        assert_eq!(deserialized.subtasks[0].title, "First subtask");
        assert_eq!(deserialized.subtasks[0].files, vec!["src/main.rs"]);
        assert_eq!(deserialized.subtasks[0].acceptance_criteria.len(), 1);
        assert!(!deserialized.subtasks[0].acceptance_criteria[0].met);
    }

    #[test]
    fn test_acceptance_criteria_default_met() {
        let toml_str = r#"description = "Tests pass""#;
        let ac: AcceptanceCriteria = toml::from_str(toml_str).unwrap();
        assert!(!ac.met);
    }
}
```

**Step 2: Update lib.rs to export the new module**

In `crates/surge-core/src/lib.rs`, add:

```rust
pub mod spec;

pub use spec::{AcceptanceCriteria, Complexity, Spec, Subtask};
```

**Step 3: Run tests to verify they pass**

Run: `cargo test -p surge-core -- spec`
Expected: 3 tests PASS

**Step 4: Commit**

```bash
git add crates/surge-core/src/spec.rs crates/surge-core/src/lib.rs
git commit -m "feat(core): add spec types — Spec, Subtask, AcceptanceCriteria, Complexity"
```

---

### Task 2: Add `event.rs` to surge-core

**Files:**
- Create: `crates/surge-core/src/event.rs`
- Modify: `crates/surge-core/src/lib.rs`
- Modify: `crates/surge-acp/src/client.rs` (remove SurgeEvent, import from core)
- Modify: `crates/surge-acp/src/lib.rs` (re-export from core instead)

**Step 1: Create event.rs in surge-core**

```rust
//! Events emitted throughout the Surge system.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::id::{SpecId, SubtaskId, TaskId};
use crate::state::TaskState;

/// Events emitted by Surge for monitoring, UI updates, and logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SurgeEvent {
    // --- Agent events ---
    /// Agent connected successfully.
    AgentConnected { agent_name: String },

    /// Agent disconnected.
    AgentDisconnected { agent_name: String },

    /// Agent requested a permission.
    PermissionRequested { description: String },

    /// Permission was granted or denied.
    PermissionResolved { granted: bool },

    // --- Task lifecycle events ---
    /// Task state changed.
    TaskStateChanged {
        task_id: TaskId,
        old_state: TaskState,
        new_state: TaskState,
    },

    /// Subtask started execution.
    SubtaskStarted {
        task_id: TaskId,
        subtask_id: SubtaskId,
    },

    /// Subtask completed.
    SubtaskCompleted {
        task_id: TaskId,
        subtask_id: SubtaskId,
        success: bool,
    },

    // --- File events ---
    /// File operation performed by agent.
    FileOperation { operation: String, path: PathBuf },

    // --- Terminal events ---
    /// Terminal command executed.
    TerminalCommand {
        command: String,
        exit_code: Option<i32>,
    },

    // --- Spec events ---
    /// Spec was loaded or created.
    SpecLoaded { spec_id: SpecId },
}
```

**Step 2: Update surge-core lib.rs**

Add to `crates/surge-core/src/lib.rs`:

```rust
pub mod event;

pub use event::SurgeEvent;
```

**Step 3: Migrate surge-acp to use surge-core::SurgeEvent**

In `crates/surge-acp/src/client.rs`:
- Remove the `SurgeEvent` enum definition (lines 57-73)
- Add `use surge_core::SurgeEvent;` import
- Update `emit_event` calls to use new variant names (PermissionRequested/PermissionResolved/FileOperation/TerminalCommand stay the same)

In `crates/surge-acp/src/lib.rs`:
- Change `pub use client::{..., SurgeEvent}` to `pub use surge_core::SurgeEvent;`

**Step 4: Run tests**

Run: `cargo test --workspace`
Expected: All tests PASS

**Step 5: Commit**

```bash
git add crates/surge-core/src/event.rs crates/surge-core/src/lib.rs crates/surge-acp/src/client.rs crates/surge-acp/src/lib.rs
git commit -m "refactor(core): move SurgeEvent to surge-core, add task lifecycle events"
```

---

### Task 3: Extend SurgeConfig with RoutingConfig, CleanupPolicy, IdeConfig

**Files:**
- Modify: `crates/surge-core/src/config.rs`

**Step 1: Write the failing test**

Add to the `tests` module in `crates/surge-core/src/config.rs`:

```rust
#[test]
fn test_routing_config_defaults() {
    let config = RoutingConfig::default();
    assert_eq!(config.strategy, RoutingStrategy::Default);
    assert!(config.agent_preferences.is_empty());
}

#[test]
fn test_cleanup_policy_defaults() {
    let policy = CleanupPolicy::default();
    assert!(policy.remove_worktrees_on_complete);
    assert_eq!(policy.keep_branches_days, 7);
}

#[test]
fn test_extended_config_toml_roundtrip() {
    let toml_str = r#"
default_agent = "claude"

[agents.claude]
command = "claude"

[pipeline]
max_qa_iterations = 10
max_parallel = 3

[routing]
strategy = "default"

[cleanup]
remove_worktrees_on_complete = false
keep_branches_days = 14

[ide]
editor = "rustrover"
"#;
    let config: SurgeConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.routing.strategy, RoutingStrategy::Default);
    assert!(!config.cleanup.remove_worktrees_on_complete);
    assert_eq!(config.cleanup.keep_branches_days, 14);
    assert_eq!(config.ide.editor, Some("rustrover".to_string()));
}

#[test]
fn test_extended_config_missing_sections_use_defaults() {
    let toml_str = r#"default_agent = "test""#;
    let config: SurgeConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.routing.strategy, RoutingStrategy::Default);
    assert!(config.cleanup.remove_worktrees_on_complete);
    assert_eq!(config.cleanup.keep_branches_days, 7);
    assert!(config.ide.editor.is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p surge-core -- test_routing_config`
Expected: FAIL — RoutingConfig not defined

**Step 3: Add the types to config.rs**

Add before the `impl Default for SurgeConfig` block:

```rust
/// Strategy for routing tasks to agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RoutingStrategy {
    /// Use the default agent for all tasks.
    #[default]
    Default,
    /// Route based on task complexity.
    Complexity,
    /// Round-robin across available agents.
    RoundRobin,
}

/// Configuration for agent routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Routing strategy.
    #[serde(default)]
    pub strategy: RoutingStrategy,
    /// Per-complexity agent preferences (e.g. {"complex": "claude"}).
    #[serde(default)]
    pub agent_preferences: HashMap<String, String>,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            strategy: RoutingStrategy::Default,
            agent_preferences: HashMap::new(),
        }
    }
}

/// Policy for cleaning up git worktrees and branches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupPolicy {
    /// Remove worktrees when task completes.
    #[serde(default = "default_true")]
    pub remove_worktrees_on_complete: bool,
    /// Days to keep merged branches before cleanup.
    #[serde(default = "default_keep_branches_days")]
    pub keep_branches_days: u32,
}

impl Default for CleanupPolicy {
    fn default() -> Self {
        Self {
            remove_worktrees_on_complete: true,
            keep_branches_days: default_keep_branches_days(),
        }
    }
}

fn default_keep_branches_days() -> u32 {
    7
}

/// IDE integration configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IdeConfig {
    /// Editor name (e.g. "vscode", "rustrover", "zed").
    #[serde(default)]
    pub editor: Option<String>,
}
```

Then extend `SurgeConfig` struct:

```rust
pub struct SurgeConfig {
    pub default_agent: String,
    #[serde(default)]
    pub agents: HashMap<String, AgentConfig>,
    #[serde(default)]
    pub pipeline: PipelineConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub cleanup: CleanupPolicy,
    #[serde(default)]
    pub ide: IdeConfig,
}
```

Update `Default for SurgeConfig`:

```rust
impl Default for SurgeConfig {
    fn default() -> Self {
        Self {
            default_agent: "claude-code".to_string(),
            agents: HashMap::new(),
            pipeline: PipelineConfig::default(),
            routing: RoutingConfig::default(),
            cleanup: CleanupPolicy::default(),
            ide: IdeConfig::default(),
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p surge-core`
Expected: All tests PASS (including existing ones — the new fields have defaults so old TOML still deserializes)

**Step 5: Commit**

```bash
git add crates/surge-core/src/config.rs
git commit -m "feat(core): add RoutingConfig, CleanupPolicy, IdeConfig to SurgeConfig"
```

---

### Task 4: Wire CLI `ping` command to ACP

**Files:**
- Modify: `crates/surge-cli/src/main.rs`

**Step 1: Write the implementation**

Replace the `Commands::Ping` handler in `main.rs`:

```rust
Commands::Ping { agent } => {
    let mut config = SurgeConfig::load_or_default()?;
    config.apply_env_overrides();

    let agent_name = agent.as_deref().unwrap_or(&config.default_agent);

    if !config.agents.contains_key(agent_name) {
        anyhow::bail!("Agent '{}' not found in configuration", agent_name);
    }

    println!("⚡ Pinging agent '{agent_name}'...");

    let cwd = std::env::current_dir()?;
    let pool = surge_acp::AgentPool::new(
        config.agents.clone(),
        config.default_agent.clone(),
        cwd,
        surge_acp::PermissionPolicy::default(),
    )?;

    match pool.ping(agent_name).await {
        Ok(()) => {
            println!("✅ Agent '{agent_name}' is responsive");
        }
        Err(e) => {
            println!("❌ Agent '{agent_name}' failed: {e}");
            std::process::exit(1);
        }
    }

    pool.shutdown().await;
}
```

**Step 2: Run build check**

Run: `cargo check -p surge-cli`
Expected: Compiles

**Step 3: Commit**

```bash
git add crates/surge-cli/src/main.rs
git commit -m "feat(cli): wire ping command to ACP AgentPool"
```

---

### Task 5: Wire CLI `prompt` command to ACP

**Files:**
- Modify: `crates/surge-cli/src/main.rs`

**Step 1: Write the implementation**

Replace the `Commands::Prompt` handler:

```rust
Commands::Prompt { message, agent } => {
    let mut config = SurgeConfig::load_or_default()?;
    config.apply_env_overrides();

    let agent_name = agent.as_deref().unwrap_or(&config.default_agent);

    if !config.agents.contains_key(agent_name) {
        anyhow::bail!("Agent '{}' not found in configuration", agent_name);
    }

    println!("⚡ Sending to '{agent_name}': {message}");

    let cwd = std::env::current_dir()?;
    let pool = surge_acp::AgentPool::new(
        config.agents.clone(),
        config.default_agent.clone(),
        cwd.clone(),
        surge_acp::PermissionPolicy::default(),
    )?;

    let session = pool.create_session(Some(agent_name), None, &cwd).await?;

    let content = vec![agent_client_protocol::ContentBlock::Text(
        agent_client_protocol::TextContent {
            text: message,
            meta: None,
        },
    )];

    let response = pool.prompt(&session, content).await?;

    // Print response content blocks
    for block in &response.content {
        match block {
            agent_client_protocol::ContentBlock::Text(text) => {
                println!("{}", text.text);
            }
            _ => {
                println!("[non-text content block]");
            }
        }
    }

    pool.shutdown().await;
}
```

**Step 2: Add agent-client-protocol dependency to surge-cli**

In `crates/surge-cli/Cargo.toml` add:

```toml
agent-client-protocol = { workspace = true }
```

**Step 3: Add import in main.rs**

At the top of `main.rs`, the existing imports suffice since we use fully qualified paths. But we do need agent_client_protocol in scope. No new `use` needed if we use full paths.

Actually we use `agent_client_protocol::ContentBlock` and `agent_client_protocol::TextContent` — so either add the dep and use full paths, or add a use statement. Full paths are fine.

**Step 4: Run build check**

Run: `cargo check -p surge-cli`
Expected: Compiles

**Step 5: Commit**

```bash
git add crates/surge-cli/Cargo.toml crates/surge-cli/src/main.rs
git commit -m "feat(cli): wire prompt command to ACP — send prompt, display response"
```

---

### Task 6: Wire CLI `agent test` command to ACP

**Files:**
- Modify: `crates/surge-cli/src/main.rs`

**Step 1: Write the implementation**

Replace the `AgentCommands::Test` handler:

```rust
AgentCommands::Test { name } => {
    let mut config = SurgeConfig::load_or_default()?;
    config.apply_env_overrides();

    if !config.agents.contains_key(&name) {
        anyhow::bail!("Agent '{}' not found in configuration", name);
    }

    println!("⚡ Testing agent '{name}'...");

    let agent_config = config.agents.get(&name).unwrap();
    println!("   Command: {}", agent_config.command);
    if !agent_config.args.is_empty() {
        println!("   Args: {:?}", agent_config.args);
    }

    let cwd = std::env::current_dir()?;
    let pool = surge_acp::AgentPool::new(
        config.agents.clone(),
        config.default_agent.clone(),
        cwd,
        surge_acp::PermissionPolicy::default(),
    )?;

    match pool.ping(&name).await {
        Ok(()) => {
            println!("✅ Agent '{name}' — connection OK");
        }
        Err(e) => {
            println!("❌ Agent '{name}' — failed: {e}");
            std::process::exit(1);
        }
    }

    pool.shutdown().await;
}
```

**Step 2: Run build check**

Run: `cargo check -p surge-cli`
Expected: Compiles

**Step 3: Commit**

```bash
git add crates/surge-cli/src/main.rs
git commit -m "feat(cli): wire agent test command to ACP ping"
```

---

### Task 7: Add `init` command

**Files:**
- Modify: `crates/surge-cli/src/main.rs`

**Step 1: Add Init to Commands enum**

```rust
/// Initialize surge.toml in current directory
Init,
```

**Step 2: Write the handler**

```rust
Commands::Init => {
    let config_path = std::env::current_dir()?.join("surge.toml");

    if config_path.exists() {
        anyhow::bail!("surge.toml already exists in current directory");
    }

    let default_toml = r#"# Surge configuration
# See: https://github.com/vanyastaff/surge

default_agent = "claude"

[agents.claude]
command = "claude"
args = ["--print", "--output-format", "stream-json"]
transport = "stdio"

[pipeline]
max_qa_iterations = 10
max_parallel = 3

[pipeline.gates]
after_spec = true
after_plan = true
after_each_subtask = false
after_qa = true
"#;

    std::fs::write(&config_path, default_toml)?;
    println!("⚡ Created surge.toml");
    println!("   Edit agents section to configure your coding agents.");
}
```

**Step 3: Run build check**

Run: `cargo check -p surge-cli`
Expected: Compiles

**Step 4: Commit**

```bash
git add crates/surge-cli/src/main.rs
git commit -m "feat(cli): add init command to generate default surge.toml"
```

---

### Task 8: Final verification and cleanup

**Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests PASS

**Step 2: Run clippy**

Run: `cargo clippy --workspace`
Expected: No warnings

**Step 3: Verify surge.example.toml still deserializes**

Write a test in `crates/surge-core/src/config.rs`:

```rust
#[test]
fn test_example_toml_deserializes() {
    let content = include_str!("../../../surge.example.toml");
    let config: SurgeConfig = toml::from_str(content).unwrap();
    assert_eq!(config.default_agent, "claude");
    assert!(config.agents.contains_key("claude"));
    config.validate().unwrap();
}
```

**Step 4: Run tests**

Run: `cargo test -p surge-core -- test_example_toml`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "test(core): add surge.example.toml deserialization test, verify Phase 0"
```

---

## Dependency Graph

```
Task 1 (spec.rs) ──────┐
                        ├──→ Task 2 (event.rs, uses SpecId) ──→ Task 8 (final)
Task 3 (config ext) ────┤
                        ├──→ Task 4 (ping) ──→ Task 5 (prompt) ──→ Task 6 (agent test) ──→ Task 7 (init) ──→ Task 8
                        │
                        └──→ (Tasks 4-7 can start after Task 3 since config changes are backward-compatible)
```

Parallel tracks:
- **Track A:** Tasks 1 → 2 (core types)
- **Track B:** Tasks 3 → 4 → 5 → 6 → 7 (config + CLI)
- **Merge:** Task 8 (verification)

Tasks 1 and 3 are independent and can be done in parallel.
Tasks 4-7 depend on Task 3 (extended config must compile).
Task 2 depends on Task 1 (uses SpecId from spec.rs).
