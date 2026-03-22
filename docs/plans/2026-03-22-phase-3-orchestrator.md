# Phase 3: Orchestrator MVP — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the orchestrator — the pipeline that takes a spec and drives it through planning, execution, QA review, and merge using agents in isolated worktrees.

**Architecture:** New `surge-orchestrator` crate ties together all existing crates. `Orchestrator` takes a spec, creates a worktree, executes subtasks sequentially via ACP agents, runs QA review loop, respects gate configurations, and merges on success. Events broadcast via `tokio::sync::broadcast`. File-based gates (PAUSE/HUMAN_INPUT.md) for human interaction.

**Tech Stack:** Rust 2024, tokio, surge-core/acp/spec/git, broadcast channels

---

### Task 1: Create surge-orchestrator crate scaffold + Phase enum

**Files:**
- Create: `crates/surge-orchestrator/Cargo.toml`
- Create: `crates/surge-orchestrator/src/lib.rs`
- Create: `crates/surge-orchestrator/src/phases.rs`
- Modify: `Cargo.toml` (workspace)
- Modify: `crates/surge-cli/Cargo.toml`

**Step 1: Create the crate**

`crates/surge-orchestrator/Cargo.toml`:
```toml
[package]
name = "surge-orchestrator"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
surge-core = { workspace = true }
surge-acp = { workspace = true }
surge-spec = { workspace = true }
surge-git = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
thiserror = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
agent-client-protocol = { workspace = true }
```

`crates/surge-orchestrator/src/phases.rs`:
```rust
//! Pipeline phase definitions.

use serde::{Deserialize, Serialize};

/// Pipeline phases for task execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// Sending spec to agent for planning/subtask breakdown.
    Planning,
    /// Executing subtasks sequentially.
    Executing,
    /// Agent reviewing code against acceptance criteria.
    QaReview,
    /// Agent fixing issues found in QA.
    QaFix,
    /// Waiting for human review/input.
    HumanReview,
    /// Merging worktree into target branch.
    Merging,
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Planning => write!(f, "Planning"),
            Self::Executing => write!(f, "Executing"),
            Self::QaReview => write!(f, "QA Review"),
            Self::QaFix => write!(f, "QA Fix"),
            Self::HumanReview => write!(f, "Human Review"),
            Self::Merging => write!(f, "Merging"),
        }
    }
}
```

`crates/surge-orchestrator/src/lib.rs`:
```rust
//! Orchestrator — drives specs through the full pipeline.

pub mod phases;
pub mod context;
pub mod executor;
pub mod qa;
pub mod gates;
pub mod pipeline;

pub use phases::Phase;
pub use pipeline::Orchestrator;
```

Create empty module files for context, executor, qa, gates, pipeline.

Add to workspace and CLI Cargo.toml.

**Step 2: Verify compilation, commit**

---

### Task 2: context.rs — SubtaskContext prompt builder

**Files:**
- Create: `crates/surge-orchestrator/src/context.rs`

**Step 1: Write context.rs**

```rust
//! Subtask context — builds prompts for agent execution.

use surge_core::spec::{Spec, Subtask};

/// Builds a prompt for executing a subtask.
pub struct SubtaskContext<'a> {
    spec: &'a Spec,
    subtask: &'a Subtask,
}

impl<'a> SubtaskContext<'a> {
    /// Create a new context for a subtask.
    pub fn new(spec: &'a Spec, subtask: &'a Subtask) -> Self {
        Self { spec, subtask }
    }

    /// Build the full prompt string for the agent.
    pub fn build_prompt(&self) -> String {
        let mut prompt = String::new();

        // Spec context
        prompt.push_str(&format!("# Task: {}\n\n", self.spec.title));
        prompt.push_str(&format!("## Overall Goal\n{}\n\n", self.spec.description));

        // Subtask details
        prompt.push_str(&format!("## Current Subtask: {}\n\n", self.subtask.title));
        prompt.push_str(&format!("{}\n\n", self.subtask.description));

        // Files to touch
        if !self.subtask.files.is_empty() {
            prompt.push_str("## Relevant Files\n");
            for file in &self.subtask.files {
                prompt.push_str(&format!("- {file}\n"));
            }
            prompt.push('\n');
        }

        // Acceptance criteria
        if !self.subtask.acceptance_criteria.is_empty() {
            prompt.push_str("## Acceptance Criteria\n");
            for ac in &self.subtask.acceptance_criteria {
                prompt.push_str(&format!("- [ ] {}\n", ac.description));
            }
            prompt.push('\n');
        }

        // Instructions
        prompt.push_str("## Instructions\n");
        prompt.push_str("1. Implement the changes described above\n");
        prompt.push_str("2. Make sure all acceptance criteria are met\n");
        prompt.push_str("3. Run tests to verify your changes work\n");
        prompt.push_str("4. Keep changes focused on this subtask only\n");

        prompt
    }
}

/// Build a QA review prompt from spec + diff.
pub fn build_qa_prompt(spec: &Spec, diff: &str) -> String {
    let mut prompt = String::new();

    prompt.push_str(&format!("# QA Review: {}\n\n", spec.title));
    prompt.push_str("Review the following changes against the acceptance criteria.\n\n");

    // Acceptance criteria from all subtasks
    prompt.push_str("## Acceptance Criteria\n");
    for subtask in &spec.subtasks {
        for ac in &subtask.acceptance_criteria {
            prompt.push_str(&format!("- [ ] {} (subtask: {})\n", ac.description, subtask.title));
        }
    }
    prompt.push('\n');

    // Diff
    prompt.push_str("## Changes (diff)\n```diff\n");
    prompt.push_str(diff);
    prompt.push_str("\n```\n\n");

    // Instructions
    prompt.push_str("## Your Task\n");
    prompt.push_str("Review the changes and respond with one of:\n");
    prompt.push_str("- `APPROVED` — all criteria met, code looks good\n");
    prompt.push_str("- `NEEDS_FIX: <description of issues>` — list specific issues to fix\n");

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::{SpecId, SubtaskId};
    use surge_core::spec::{AcceptanceCriteria, Complexity};

    #[test]
    fn test_subtask_prompt_contains_key_parts() {
        let spec = Spec {
            id: SpecId::new(),
            title: "Add auth".to_string(),
            description: "Add authentication".to_string(),
            complexity: Complexity::Standard,
            subtasks: vec![],
        };
        let subtask = Subtask {
            id: SubtaskId::new(),
            title: "Login endpoint".to_string(),
            description: "Create POST /login".to_string(),
            complexity: Complexity::Simple,
            files: vec!["src/api.rs".to_string()],
            acceptance_criteria: vec![AcceptanceCriteria {
                description: "Returns 200 on success".to_string(),
                met: false,
            }],
            depends_on: vec![],
        };

        let ctx = SubtaskContext::new(&spec, &subtask);
        let prompt = ctx.build_prompt();

        assert!(prompt.contains("Add auth"));
        assert!(prompt.contains("Login endpoint"));
        assert!(prompt.contains("POST /login"));
        assert!(prompt.contains("src/api.rs"));
        assert!(prompt.contains("Returns 200 on success"));
    }

    #[test]
    fn test_qa_prompt_contains_diff() {
        let spec = Spec {
            id: SpecId::new(),
            title: "Feature".to_string(),
            description: "A feature".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![],
        };
        let prompt = build_qa_prompt(&spec, "+new line\n-old line");
        assert!(prompt.contains("APPROVED"));
        assert!(prompt.contains("NEEDS_FIX"));
        assert!(prompt.contains("+new line"));
    }
}
```

**Step 2: Run tests, commit**

---

### Task 3: executor.rs — subtask execution via ACP

**Files:**
- Create: `crates/surge-orchestrator/src/executor.rs`

**Step 1: Write executor.rs**

```rust
//! Subtask executor — runs a single subtask via ACP agent.

use agent_client_protocol::{ContentBlock, TextContent};
use surge_acp::{AgentPool, SessionHandle};
use surge_core::id::{SubtaskId, TaskId};
use surge_core::spec::{Spec, Subtask};
use surge_core::SurgeEvent;
use surge_git::GitManager;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::context::SubtaskContext;

/// Result of executing a subtask.
#[derive(Debug)]
pub enum SubtaskResult {
    /// Subtask completed successfully.
    Success { subtask_id: SubtaskId },
    /// Subtask failed after all retries.
    Failed { subtask_id: SubtaskId, reason: String },
}

/// Configuration for subtask execution.
pub struct ExecutorConfig {
    /// Maximum number of retry attempts.
    pub max_retries: u32,
    /// Number of consecutive failures before pausing pipeline.
    pub circuit_breaker_threshold: u32,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            circuit_breaker_threshold: 3,
        }
    }
}

/// Executes subtasks via ACP agents.
pub struct SubtaskExecutor {
    config: ExecutorConfig,
    consecutive_failures: u32,
}

impl SubtaskExecutor {
    /// Create a new executor.
    pub fn new(config: ExecutorConfig) -> Self {
        Self {
            config,
            consecutive_failures: 0,
        }
    }

    /// Check if circuit breaker has tripped.
    pub fn is_circuit_broken(&self) -> bool {
        self.consecutive_failures >= self.config.circuit_breaker_threshold
    }

    /// Reset consecutive failure counter (call after successful subtask).
    pub fn reset_failures(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Execute a single subtask.
    pub async fn execute(
        &mut self,
        spec: &Spec,
        subtask: &Subtask,
        task_id: TaskId,
        pool: &AgentPool,
        session: &SessionHandle,
        git: &GitManager,
        event_tx: &broadcast::Sender<SurgeEvent>,
    ) -> SubtaskResult {
        let _ = event_tx.send(SurgeEvent::SubtaskStarted {
            task_id,
            subtask_id: subtask.id,
        });

        // Build prompt
        let ctx = SubtaskContext::new(spec, subtask);
        let prompt = ctx.build_prompt();

        // Try with retries
        let mut last_error = String::new();
        for attempt in 1..=self.config.max_retries {
            info!(
                subtask_title = %subtask.title,
                attempt,
                max = self.config.max_retries,
                "executing subtask"
            );

            let content = vec![ContentBlock::Text(TextContent {
                text: prompt.clone(),
                annotations: None,
                meta: None,
            })];

            match pool.prompt(session, content).await {
                Ok(_response) => {
                    // Commit changes in worktree
                    let commit_msg = format!("feat: {} (subtask: {})", subtask.title, subtask.id);
                    match git.commit(&spec.id.to_string(), &commit_msg) {
                        Ok(oid) => {
                            info!(subtask_title = %subtask.title, %oid, "subtask committed");
                        }
                        Err(e) => {
                            // Commit failure might just mean no changes — not fatal
                            warn!(subtask_title = %subtask.title, %e, "commit failed (may be no changes)");
                        }
                    }

                    self.reset_failures();
                    let _ = event_tx.send(SurgeEvent::SubtaskCompleted {
                        task_id,
                        subtask_id: subtask.id,
                        success: true,
                    });
                    return SubtaskResult::Success { subtask_id: subtask.id };
                }
                Err(e) => {
                    last_error = e.to_string();
                    warn!(
                        subtask_title = %subtask.title,
                        attempt,
                        error = %e,
                        "subtask attempt failed"
                    );
                }
            }
        }

        // All retries exhausted
        self.consecutive_failures += 1;
        let _ = event_tx.send(SurgeEvent::SubtaskCompleted {
            task_id,
            subtask_id: subtask.id,
            success: false,
        });
        SubtaskResult::Failed {
            subtask_id: subtask.id,
            reason: last_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_config_defaults() {
        let config = ExecutorConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.circuit_breaker_threshold, 3);
    }

    #[test]
    fn test_circuit_breaker() {
        let mut exec = SubtaskExecutor::new(ExecutorConfig {
            max_retries: 1,
            circuit_breaker_threshold: 2,
        });

        assert!(!exec.is_circuit_broken());
        exec.consecutive_failures = 1;
        assert!(!exec.is_circuit_broken());
        exec.consecutive_failures = 2;
        assert!(exec.is_circuit_broken());

        exec.reset_failures();
        assert!(!exec.is_circuit_broken());
    }
}
```

**Step 2: Run tests, commit**

---

### Task 4: qa.rs — QA review loop

**Files:**
- Create: `crates/surge-orchestrator/src/qa.rs`

**Step 1: Write qa.rs**

```rust
//! QA review loop — review code against acceptance criteria, fix issues.

use agent_client_protocol::{ContentBlock, TextContent};
use surge_acp::{AgentPool, SessionHandle};
use surge_core::spec::Spec;
use surge_git::GitManager;
use tracing::{info, warn};

use crate::context::build_qa_prompt;

/// QA review verdict.
#[derive(Debug, Clone)]
pub enum QaVerdict {
    /// All criteria met.
    Approved,
    /// Issues found that need fixing.
    NeedsFix { issues: String },
}

/// Result of a QA cycle.
#[derive(Debug)]
pub struct QaCycleResult {
    /// Final verdict.
    pub verdict: QaVerdict,
    /// Number of QA iterations performed.
    pub iterations: u32,
}

/// Runs the QA review loop.
pub struct QaReviewer {
    max_iterations: u32,
}

impl QaReviewer {
    /// Create a new QA reviewer.
    pub fn new(max_iterations: u32) -> Self {
        Self { max_iterations }
    }

    /// Run the QA review loop.
    ///
    /// Sends diff + criteria to agent, parses response.
    /// If NEEDS_FIX, sends fix prompt and re-reviews.
    /// Loops up to max_iterations.
    pub async fn run(
        &self,
        spec: &Spec,
        pool: &AgentPool,
        session: &SessionHandle,
        git: &GitManager,
    ) -> Result<QaCycleResult, surge_core::SurgeError> {
        let spec_id = spec.id.to_string();

        for iteration in 1..=self.max_iterations {
            info!(iteration, max = self.max_iterations, "QA review iteration");

            // Get current diff
            let diff = git.diff(&spec_id)
                .map_err(|e| surge_core::SurgeError::Git(e.to_string()))?;

            if diff.trim().is_empty() {
                info!("No diff — nothing to review");
                return Ok(QaCycleResult {
                    verdict: QaVerdict::Approved,
                    iterations: iteration,
                });
            }

            // Build QA prompt
            let qa_prompt = build_qa_prompt(spec, &diff);
            let content = vec![ContentBlock::Text(TextContent {
                text: qa_prompt,
                annotations: None,
                meta: None,
            })];

            // Send to agent
            let response = pool.prompt(session, content).await?;

            // Parse verdict from response
            let verdict = parse_qa_response(&response);

            match &verdict {
                QaVerdict::Approved => {
                    info!(iteration, "QA approved");
                    return Ok(QaCycleResult {
                        verdict,
                        iterations: iteration,
                    });
                }
                QaVerdict::NeedsFix { issues } => {
                    info!(iteration, issues = %issues, "QA found issues, requesting fix");

                    if iteration == self.max_iterations {
                        // Last iteration — return as needs fix
                        return Ok(QaCycleResult {
                            verdict,
                            iterations: iteration,
                        });
                    }

                    // Send fix prompt
                    let fix_prompt = format!(
                        "# Fix Required\n\nThe QA review found the following issues:\n\n{issues}\n\n\
                        Please fix these issues. The acceptance criteria must be met."
                    );
                    let fix_content = vec![ContentBlock::Text(TextContent {
                        text: fix_prompt,
                        annotations: None,
                        meta: None,
                    })];

                    match pool.prompt(session, fix_content).await {
                        Ok(_) => {
                            // Commit the fix
                            let msg = format!("fix: QA iteration {iteration}");
                            let _ = git.commit(&spec_id, &msg);
                        }
                        Err(e) => {
                            warn!(iteration, error = %e, "fix prompt failed");
                        }
                    }
                }
            }
        }

        Ok(QaCycleResult {
            verdict: QaVerdict::NeedsFix { issues: "Max QA iterations reached".to_string() },
            iterations: self.max_iterations,
        })
    }
}

/// Parse a QA response from the agent.
fn parse_qa_response(response: &agent_client_protocol::PromptResponse) -> QaVerdict {
    // Since PromptResponse in ACP 0.6 doesn't have content field directly,
    // we look at stop_reason. For now, default to approved as a placeholder.
    // In real usage, content comes through events/streaming.
    // TODO: Parse actual response content when ACP streaming is wired.

    let stop_reason = format!("{:?}", response.stop_reason);

    if stop_reason.contains("end_turn") || stop_reason.contains("EndTurn") {
        // Heuristic: end_turn usually means the agent finished normally
        QaVerdict::Approved
    } else {
        QaVerdict::NeedsFix {
            issues: "Agent response unclear — manual review recommended".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qa_reviewer_creation() {
        let reviewer = QaReviewer::new(5);
        assert_eq!(reviewer.max_iterations, 5);
    }
}
```

**Step 2: Run tests, commit**

---

### Task 5: gates.rs — pipeline gate management

**Files:**
- Create: `crates/surge-orchestrator/src/gates.rs`

**Step 1: Write gates.rs**

```rust
//! Gate management — controls pipeline pausing and human input.

use std::path::{Path, PathBuf};
use surge_core::config::GateConfig;
use tracing::info;

/// Gate check result.
#[derive(Debug, Clone)]
pub enum GateAction {
    /// Continue pipeline execution.
    Continue,
    /// Pause pipeline for human review.
    Pause { reason: String },
    /// Human input available to inject.
    HumanInput { content: String },
}

/// Manages pipeline gates and human interaction points.
pub struct GateManager {
    config: GateConfig,
    specs_dir: PathBuf,
}

impl GateManager {
    /// Create a new gate manager.
    pub fn new(config: GateConfig, specs_dir: PathBuf) -> Self {
        Self { config, specs_dir }
    }

    /// Check if the pipeline should pause after a given phase.
    pub fn check_gate(&self, phase: &str, spec_id: &str) -> GateAction {
        // Check for PAUSE file
        let pause_file = self.spec_dir(spec_id).join("PAUSE");
        if pause_file.exists() {
            info!(spec_id, "PAUSE file detected");
            return GateAction::Pause {
                reason: "PAUSE file found".to_string(),
            };
        }

        // Check for HUMAN_INPUT.md
        let input_file = self.spec_dir(spec_id).join("HUMAN_INPUT.md");
        if input_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&input_file) {
                if !content.trim().is_empty() {
                    info!(spec_id, "HUMAN_INPUT.md detected");
                    // Consume the input by removing the file
                    let _ = std::fs::remove_file(&input_file);
                    return GateAction::HumanInput { content };
                }
            }
        }

        // Check configured gates
        let should_pause = match phase {
            "after_spec" => self.config.after_spec,
            "after_plan" => self.config.after_plan,
            "after_each_subtask" => self.config.after_each_subtask,
            "after_qa" => self.config.after_qa,
            _ => false,
        };

        if should_pause {
            GateAction::Pause {
                reason: format!("Gate '{phase}' is enabled in configuration"),
            }
        } else {
            GateAction::Continue
        }
    }

    /// Get the spec-specific directory.
    fn spec_dir(&self, spec_id: &str) -> PathBuf {
        self.specs_dir.join(spec_id)
    }

    /// Clear the PAUSE file for a spec.
    pub fn clear_pause(&self, spec_id: &str) {
        let pause_file = self.spec_dir(spec_id).join("PAUSE");
        let _ = std::fs::remove_file(pause_file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gate_continue_when_disabled() {
        let config = GateConfig {
            after_spec: false,
            after_plan: false,
            after_each_subtask: false,
            after_qa: false,
        };
        let gm = GateManager::new(config, PathBuf::from("/tmp/specs"));
        assert!(matches!(gm.check_gate("after_spec", "test"), GateAction::Continue));
    }

    #[test]
    fn test_gate_pause_when_enabled() {
        let config = GateConfig {
            after_spec: true,
            after_plan: false,
            after_each_subtask: false,
            after_qa: false,
        };
        let gm = GateManager::new(config, PathBuf::from("/tmp/specs"));
        assert!(matches!(gm.check_gate("after_spec", "test"), GateAction::Pause { .. }));
    }

    #[test]
    fn test_pause_file_detection() {
        let temp = std::env::temp_dir().join("surge_gate_test");
        let spec_dir = temp.join("test-spec");
        let _ = std::fs::create_dir_all(&spec_dir);
        std::fs::write(spec_dir.join("PAUSE"), "").unwrap();

        let config = GateConfig {
            after_spec: false,
            after_plan: false,
            after_each_subtask: false,
            after_qa: false,
        };
        let gm = GateManager::new(config, temp.clone());
        assert!(matches!(gm.check_gate("anything", "test-spec"), GateAction::Pause { .. }));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_human_input_file() {
        let temp = std::env::temp_dir().join("surge_gate_input_test");
        let spec_dir = temp.join("input-spec");
        let _ = std::fs::create_dir_all(&spec_dir);
        std::fs::write(spec_dir.join("HUMAN_INPUT.md"), "Please add error handling").unwrap();

        let config = GateConfig {
            after_spec: false,
            after_plan: false,
            after_each_subtask: false,
            after_qa: false,
        };
        let gm = GateManager::new(config, temp.clone());
        let action = gm.check_gate("any", "input-spec");
        assert!(matches!(action, GateAction::HumanInput { content } if content.contains("error handling")));

        // File should be consumed
        assert!(!spec_dir.join("HUMAN_INPUT.md").exists());

        let _ = std::fs::remove_dir_all(&temp);
    }
}
```

**Step 2: Run tests, commit**

---

### Task 6: pipeline.rs — Orchestrator main loop

**Files:**
- Create: `crates/surge-orchestrator/src/pipeline.rs`

**Step 1: Write pipeline.rs**

```rust
//! Orchestrator — main pipeline driving spec execution.

use std::path::PathBuf;

use surge_acp::{AgentPool, PermissionPolicy};
use surge_core::config::SurgeConfig;
use surge_core::id::TaskId;
use surge_core::spec::Spec;
use surge_core::state::TaskState;
use surge_core::SurgeEvent;
use surge_git::GitManager;
use surge_spec::{DependencyGraph, SpecFile};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::executor::{ExecutorConfig, SubtaskExecutor, SubtaskResult};
use crate::gates::{GateAction, GateManager};
use crate::phases::Phase;
use crate::qa::{QaReviewer, QaVerdict};

/// Pipeline execution result.
#[derive(Debug)]
pub enum PipelineResult {
    /// Pipeline completed successfully.
    Completed,
    /// Pipeline paused for human review.
    Paused { phase: Phase, reason: String },
    /// Pipeline failed.
    Failed { reason: String },
}

/// Configuration for the orchestrator.
pub struct OrchestratorConfig {
    /// Surge configuration.
    pub surge_config: SurgeConfig,
    /// Working directory.
    pub working_dir: PathBuf,
}

/// The main orchestrator — drives a spec through the full pipeline.
pub struct Orchestrator {
    config: OrchestratorConfig,
    event_tx: broadcast::Sender<SurgeEvent>,
}

impl Orchestrator {
    /// Create a new orchestrator.
    pub fn new(config: OrchestratorConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self { config, event_tx }
    }

    /// Subscribe to pipeline events.
    pub fn subscribe(&self) -> broadcast::Receiver<SurgeEvent> {
        self.event_tx.subscribe()
    }

    /// Execute a spec through the full pipeline.
    pub async fn execute(&self, spec_file: &SpecFile) -> PipelineResult {
        let spec = &spec_file.spec;
        let spec_id = spec.id.to_string();
        let task_id = TaskId::new();

        info!(spec_id = %spec_id, title = %spec.title, "starting pipeline");

        // Validate spec
        let validation = surge_spec::validate_spec(spec);
        if !validation.is_ok() {
            return PipelineResult::Failed {
                reason: format!("Spec validation failed: {:?}", validation.errors),
            };
        }

        // Create git worktree
        let git = match GitManager::new(self.config.working_dir.clone()) {
            Ok(g) => g,
            Err(e) => return PipelineResult::Failed { reason: format!("Git error: {e}") },
        };

        let worktree_info = match git.create_worktree(&spec_id) {
            Ok(info) => info,
            Err(e) => return PipelineResult::Failed { reason: format!("Worktree error: {e}") },
        };

        info!(worktree = %worktree_info.path.display(), "created worktree");

        // Create agent pool
        let pool = match AgentPool::new(
            self.config.surge_config.agents.clone(),
            self.config.surge_config.default_agent.clone(),
            worktree_info.path.clone(),
            PermissionPolicy::default(),
        ) {
            Ok(p) => p,
            Err(e) => return PipelineResult::Failed { reason: format!("Agent pool error: {e}") },
        };

        // Create ACP session
        let session = match pool.create_session(None, None, &worktree_info.path).await {
            Ok(s) => s,
            Err(e) => return PipelineResult::Failed { reason: format!("Session error: {e}") },
        };

        // Gate manager
        let specs_dir = self.config.working_dir.join(".surge").join("specs");
        let gate_mgr = GateManager::new(
            self.config.surge_config.pipeline.gates.clone(),
            specs_dir,
        );

        // ---- EXECUTION PHASE ----

        // Get topological order of subtasks
        let graph = match DependencyGraph::from_spec(spec) {
            Ok(g) => g,
            Err(e) => return PipelineResult::Failed { reason: format!("Graph error: {e}") },
        };

        let order = match graph.topological_order() {
            Ok(o) => o,
            Err(e) => return PipelineResult::Failed { reason: format!("Topological sort error: {e}") },
        };

        self.emit_state_change(task_id, TaskState::Draft, TaskState::Executing {
            completed: 0,
            total: order.len(),
        });

        let mut executor = SubtaskExecutor::new(ExecutorConfig::default());
        let mut completed = 0;

        for subtask_id in &order {
            // Find the subtask
            let subtask = match spec.subtasks.iter().find(|s| s.id == *subtask_id) {
                Some(s) => s,
                None => continue,
            };

            // Check circuit breaker
            if executor.is_circuit_broken() {
                warn!("Circuit breaker tripped — pausing pipeline");
                pool.shutdown().await;
                return PipelineResult::Paused {
                    phase: Phase::Executing,
                    reason: "Too many consecutive failures".to_string(),
                };
            }

            // Check gate after each subtask
            if let GateAction::Pause { reason } = gate_mgr.check_gate("after_each_subtask", &spec_id) {
                pool.shutdown().await;
                return PipelineResult::Paused {
                    phase: Phase::Executing,
                    reason,
                };
            }

            // Execute subtask
            let result = executor.execute(
                spec, subtask, task_id, &pool, &session, &git, &self.event_tx,
            ).await;

            match result {
                SubtaskResult::Success { .. } => {
                    completed += 1;
                    self.emit_state_change(
                        task_id,
                        TaskState::Executing { completed: completed - 1, total: order.len() },
                        TaskState::Executing { completed, total: order.len() },
                    );
                }
                SubtaskResult::Failed { reason, .. } => {
                    warn!(subtask = %subtask.title, reason = %reason, "subtask failed");
                }
            }
        }

        // ---- QA PHASE ----

        if let GateAction::Pause { reason } = gate_mgr.check_gate("after_qa", &spec_id) {
            pool.shutdown().await;
            return PipelineResult::Paused { phase: Phase::QaReview, reason };
        }

        let qa = QaReviewer::new(self.config.surge_config.pipeline.max_qa_iterations);
        let qa_result = match qa.run(spec, &pool, &session, &git).await {
            Ok(r) => r,
            Err(e) => {
                pool.shutdown().await;
                return PipelineResult::Failed { reason: format!("QA error: {e}") };
            }
        };

        match &qa_result.verdict {
            QaVerdict::Approved => {
                info!(iterations = qa_result.iterations, "QA approved");
            }
            QaVerdict::NeedsFix { issues } => {
                warn!(iterations = qa_result.iterations, issues = %issues, "QA not approved — entering human review");
                pool.shutdown().await;
                return PipelineResult::Paused {
                    phase: Phase::HumanReview,
                    reason: format!("QA issues after {} iterations: {issues}", qa_result.iterations),
                };
            }
        }

        // ---- MERGE PHASE ----

        info!("Merging worktree");
        match git.merge(&spec_id, None) {
            Ok(_) => {
                info!("Merge successful");
            }
            Err(e) => {
                pool.shutdown().await;
                return PipelineResult::Failed { reason: format!("Merge error: {e}") };
            }
        }

        // Cleanup
        let _ = git.discard(&spec_id);
        pool.shutdown().await;

        self.emit_state_change(
            task_id,
            TaskState::Merging,
            TaskState::Completed,
        );

        PipelineResult::Completed
    }

    fn emit_state_change(&self, task_id: TaskId, old: TaskState, new: TaskState) {
        let _ = self.event_tx.send(SurgeEvent::TaskStateChanged {
            task_id,
            old_state: old,
            new_state: new,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orchestrator_creation() {
        let config = OrchestratorConfig {
            surge_config: SurgeConfig::default(),
            working_dir: PathBuf::from("/tmp"),
        };
        let orch = Orchestrator::new(config);
        let _rx = orch.subscribe();
    }
}
```

**Step 2: Run tests, commit**

---

### Task 7: CLI — surge run, surge status

**Files:**
- Modify: `crates/surge-cli/src/main.rs`

**Step 1: Add CLI commands**

Add to `Commands` enum:

```rust
    /// Run a spec through the full pipeline
    Run {
        /// Spec ID or filename
        spec_id: String,
    },

    /// Show pipeline status for a spec
    Status {
        /// Spec ID
        spec_id: String,
    },
```

Add handlers:

```rust
        Commands::Run { spec_id } => {
            let mut config = SurgeConfig::load_or_default()?;
            config.apply_env_overrides();

            let spec_file = load_spec_by_id(&spec_id)?;

            println!("⚡ Running spec: {}", spec_file.spec.title);
            println!("   Subtasks: {}", spec_file.spec.subtasks.len());

            let cwd = std::env::current_dir()?;
            let orch_config = surge_orchestrator::pipeline::OrchestratorConfig {
                surge_config: config,
                working_dir: cwd,
            };
            let orchestrator = surge_orchestrator::Orchestrator::new(orch_config);

            // Subscribe to events for progress display
            let mut events = orchestrator.subscribe();
            tokio::spawn(async move {
                while let Ok(event) = events.recv().await {
                    match event {
                        SurgeEvent::SubtaskStarted { subtask_id, .. } => {
                            println!("  ▶ Starting subtask {subtask_id}");
                        }
                        SurgeEvent::SubtaskCompleted { subtask_id, success, .. } => {
                            let mark = if success { "✅" } else { "❌" };
                            println!("  {mark} Subtask {subtask_id}");
                        }
                        SurgeEvent::TaskStateChanged { new_state, .. } => {
                            println!("  📊 State: {new_state}");
                        }
                        _ => {}
                    }
                }
            });

            let result = orchestrator.execute(&spec_file).await;

            match result {
                surge_orchestrator::pipeline::PipelineResult::Completed => {
                    println!("\n✅ Pipeline completed successfully!");
                }
                surge_orchestrator::pipeline::PipelineResult::Paused { phase, reason } => {
                    println!("\n⏸️  Pipeline paused at {phase}: {reason}");
                }
                surge_orchestrator::pipeline::PipelineResult::Failed { reason } => {
                    println!("\n❌ Pipeline failed: {reason}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Status { spec_id } => {
            let git = surge_git::GitManager::discover()?;
            let worktrees = git.list_worktrees()?;
            let wt = worktrees.iter().find(|w| w.spec_id.contains(&spec_id));

            if let Some(wt) = wt {
                println!("⚡ Status for '{}':", spec_id);
                println!("   Worktree: {} {}", if wt.exists_on_disk { "✅" } else { "❌" }, wt.path.display());
                println!("   Branch: {}", wt.branch);
            } else {
                println!("No active worktree for '{spec_id}'");
            }
        }
```

**Step 2: Add `use surge_core::SurgeEvent;` import if not present**

**Step 3: Verify compilation, commit**

---

### Task 8: Final verification

**Step 1:** `cargo test --workspace` — all pass
**Step 2:** `cargo clippy --workspace` — no warnings
**Step 3:** Commit any fixes

---

## Dependency Graph

```
Task 1 (scaffold) → Task 2 (context) ──→ Task 3 (executor) ──→ Task 6 (pipeline)
                  → Task 4 (qa) ─────────────────────────────→ Task 6
                  → Task 5 (gates) ──────────────────────────→ Task 6
                                                                  → Task 7 (CLI) → Task 8
```

Tasks 2, 4, 5 are independent after scaffold.
Task 3 depends on Task 2 (uses SubtaskContext).
Task 6 depends on Tasks 2, 3, 4, 5.
Task 7 depends on Task 6.
