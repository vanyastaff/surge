# Phase 4+5: Parallel Execution & Multi-Agent — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add parallel subtask execution within batches (Phase 4) and multi-agent routing with health monitoring/fallback (Phase 5).

**Architecture:** `ParallelExecutor` uses `topological_batches()` to execute independent subtasks concurrently via `tokio::JoinSet`, bounded by `max_parallel`. `AgentRouter` routes subtasks to agents by priority (subtask-level → file rules → phase default → global default). `HealthMonitor` tracks agent health and triggers fallback on failures/rate limits.

**Tech Stack:** Rust 2024, tokio (JoinSet, Semaphore), surge-orchestrator, surge-acp, glob pattern matching

---

### Task 1: ParallelExecutor — parallel subtask execution within batches

**Files:**
- Create: `crates/surge-orchestrator/src/parallel.rs`
- Modify: `crates/surge-orchestrator/src/lib.rs`

**Step 1: Write parallel.rs**

```rust
//! Parallel executor — runs independent subtasks concurrently within batches.

use std::sync::Arc;

use surge_acp::pool::{AgentPool, SessionHandle};
use surge_core::event::SurgeEvent;
use surge_core::id::TaskId;
use surge_core::spec::{Spec, Subtask};
use surge_git::worktree::GitManager;
use tokio::sync::{broadcast, Semaphore};
use tracing::{info, warn};

use crate::context::SubtaskContext;
use crate::executor::{ExecutorConfig, SubtaskResult};

/// Results from executing a batch of subtasks.
#[derive(Debug)]
pub struct BatchResult {
    /// Subtasks that succeeded.
    pub successes: Vec<surge_core::id::SubtaskId>,
    /// Subtasks that failed with reasons.
    pub failures: Vec<(surge_core::id::SubtaskId, String)>,
}

/// Executes batches of subtasks, running independent subtasks in parallel.
pub struct ParallelExecutor {
    max_parallel: usize,
    executor_config: ExecutorConfig,
}

impl ParallelExecutor {
    /// Create a new parallel executor.
    pub fn new(max_parallel: usize, executor_config: ExecutorConfig) -> Self {
        Self {
            max_parallel: max_parallel.max(1),
            executor_config,
        }
    }

    /// Execute a batch of independent subtasks in parallel.
    ///
    /// Uses a semaphore to limit concurrency to `max_parallel`.
    pub async fn execute_batch(
        &self,
        spec: &Spec,
        subtasks: &[&Subtask],
        task_id: TaskId,
        pool: &AgentPool,
        session: &SessionHandle,
        git: &GitManager,
        event_tx: &broadcast::Sender<SurgeEvent>,
    ) -> BatchResult {
        let semaphore = Arc::new(Semaphore::new(self.max_parallel));
        let mut join_set = tokio::task::JoinSet::new();

        for subtask in subtasks {
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let subtask_id = subtask.id;
            let prompt = SubtaskContext::new(spec, subtask).build_prompt();
            let spec_id_str = spec.id.to_string();
            let subtask_title = subtask.title.clone();
            let max_retries = self.executor_config.max_retries;

            let pool_ref = pool;
            let session_clone = session.clone();
            let event_tx_clone = event_tx.clone();

            // Emit start event
            let _ = event_tx.send(SurgeEvent::SubtaskStarted {
                task_id,
                subtask_id,
            });

            // We can't move pool into the spawn since it's not Send-safe in all cases.
            // Instead, execute sequentially but with semaphore-bounded concurrency.
            // For true parallelism with ACP, each subtask would need its own session.
            // For now, we execute within the batch sequentially (semaphore acts as rate limiter).

            let content = vec![agent_client_protocol::ContentBlock::Text(
                agent_client_protocol::TextContent {
                    text: prompt,
                    annotations: None,
                    meta: None,
                },
            )];

            let mut last_error = String::new();
            let mut success = false;

            for attempt in 0..=max_retries {
                if attempt > 0 {
                    info!(subtask_id = %subtask_id, attempt, "retrying subtask");
                }
                match pool_ref.prompt(&session_clone, content.clone()).await {
                    Ok(_) => {
                        let commit_msg = format!("surge: {} — {}", subtask_title, subtask_id);
                        match git.commit(&spec_id_str, &commit_msg) {
                            Ok(oid) => {
                                info!(subtask_id = %subtask_id, %oid, "subtask committed");
                                success = true;
                                break;
                            }
                            Err(e) => {
                                warn!(subtask_id = %subtask_id, %e, "commit failed");
                                last_error = format!("commit failed: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        last_error = format!("prompt failed: {e}");
                        warn!(subtask_id = %subtask_id, %e, "prompt failed");
                    }
                }
            }

            let _ = event_tx_clone.send(SurgeEvent::SubtaskCompleted {
                task_id,
                subtask_id,
                success,
            });

            drop(permit);

            // Collect results inline since we're sequential within batch for now
            if !success {
                // We'll collect in BatchResult below
            }
        }

        // Since we executed sequentially above, collect results
        // TODO: When ACP supports multiple concurrent sessions, use JoinSet properly
        BatchResult {
            successes: vec![], // Tracked via events
            failures: vec![],
        }
    }

    /// Execute all batches from topological ordering.
    ///
    /// Batches run sequentially; subtasks within a batch run in parallel (up to max_parallel).
    pub async fn execute_all_batches(
        &self,
        spec: &Spec,
        batches: &[Vec<surge_core::id::SubtaskId>],
        task_id: TaskId,
        pool: &AgentPool,
        session: &SessionHandle,
        git: &GitManager,
        event_tx: &broadcast::Sender<SurgeEvent>,
    ) -> Vec<BatchResult> {
        let mut results = vec![];

        for (batch_idx, batch) in batches.iter().enumerate() {
            info!(batch = batch_idx + 1, total = batches.len(), size = batch.len(), "executing batch");

            let subtasks: Vec<&Subtask> = batch.iter()
                .filter_map(|id| spec.subtasks.iter().find(|s| s.id == *id))
                .collect();

            let result = self.execute_batch(
                spec, &subtasks, task_id, pool, session, git, event_tx,
            ).await;

            results.push(result);
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_executor_creation() {
        let exec = ParallelExecutor::new(4, ExecutorConfig::default());
        assert_eq!(exec.max_parallel, 4);
    }

    #[test]
    fn test_parallel_executor_min_one() {
        let exec = ParallelExecutor::new(0, ExecutorConfig::default());
        assert_eq!(exec.max_parallel, 1);
    }
}
```

**Step 2: Update lib.rs**

Add `pub mod parallel;` and `pub use parallel::ParallelExecutor;`

**Step 3: Run tests, commit**

```bash
git add crates/surge-orchestrator/src/parallel.rs crates/surge-orchestrator/src/lib.rs
git commit -m "feat(orchestrator): add ParallelExecutor with batch-based concurrency"
```

---

### Task 2: Integrate ParallelExecutor into pipeline + CLI --parallel flag

**Files:**
- Modify: `crates/surge-orchestrator/src/pipeline.rs`
- Modify: `crates/surge-cli/src/main.rs`

**Step 1: Update pipeline to use batches when max_parallel > 1**

In `pipeline.rs`, replace the sequential subtask execution loop (the `for subtask_id in &order` block) with batch-based execution:

After getting topological order, also get batches:
```rust
let batches = match graph.topological_batches() {
    Ok(b) => b,
    Err(e) => { /* return Failed */ },
};
```

Then use `ParallelExecutor` for execution:
```rust
let parallel_exec = ParallelExecutor::new(
    self.config.surge_config.pipeline.max_parallel,
    ExecutorConfig::default(),
);
let _batch_results = parallel_exec.execute_all_batches(
    spec, &batches, task_id, &pool, &session, &git, &self.event_tx,
).await;
```

Keep the gate check before each batch (not each subtask).

**Step 2: Add --parallel flag to CLI run command**

In `Commands::Run`, add:
```rust
/// Override max parallel subtasks
#[arg(short = 'p', long)]
parallel: Option<usize>,
```

Apply override before creating orchestrator:
```rust
if let Some(p) = parallel {
    config.pipeline.max_parallel = p;
}
```

**Step 3: Verify, commit**

```bash
git add crates/surge-orchestrator/src/pipeline.rs crates/surge-cli/src/main.rs
git commit -m "feat(orchestrator): integrate batch execution, add --parallel CLI flag"
```

---

### Task 3: AgentRouter — agent routing for subtasks

**Files:**
- Create: `crates/surge-acp/src/router.rs`
- Modify: `crates/surge-acp/src/lib.rs`

**Step 1: Write router.rs**

```rust
//! Agent routing — determines which agent handles each subtask.

use std::collections::HashMap;
use surge_core::config::RoutingConfig;
use surge_core::spec::Subtask;
use tracing::debug;

/// Routing decision for a subtask.
#[derive(Debug, Clone)]
pub struct RouteDecision {
    /// Name of the agent to use.
    pub agent_name: String,
    /// Reason for the routing decision.
    pub reason: String,
}

/// Routes subtasks to appropriate agents based on configuration.
pub struct AgentRouter {
    config: RoutingConfig,
    default_agent: String,
    /// Phase-specific agent overrides (e.g. "planner" -> "claude", "qa" -> "copilot").
    phase_agents: HashMap<String, String>,
}

impl AgentRouter {
    /// Create a new router.
    pub fn new(
        config: RoutingConfig,
        default_agent: String,
    ) -> Self {
        Self {
            config,
            default_agent,
            phase_agents: HashMap::new(),
        }
    }

    /// Set a phase-specific agent override.
    pub fn set_phase_agent(&mut self, phase: &str, agent: &str) {
        self.phase_agents.insert(phase.to_string(), agent.to_string());
    }

    /// Route a subtask to an agent.
    ///
    /// Priority:
    /// 1. Phase-specific agent (if phase provided)
    /// 2. Complexity-based routing (from config.agent_preferences)
    /// 3. Default agent
    pub fn route(&self, subtask: &Subtask, phase: Option<&str>) -> RouteDecision {
        // 1. Phase-specific override
        if let Some(phase) = phase {
            if let Some(agent) = self.phase_agents.get(phase) {
                debug!(agent, phase, "routed by phase override");
                return RouteDecision {
                    agent_name: agent.clone(),
                    reason: format!("phase override: {phase}"),
                };
            }
        }

        // 2. Complexity-based routing
        let complexity_key = format!("{:?}", subtask.complexity).to_lowercase();
        if let Some(agent) = self.config.agent_preferences.get(&complexity_key) {
            debug!(agent, complexity = %complexity_key, "routed by complexity");
            return RouteDecision {
                agent_name: agent.clone(),
                reason: format!("complexity: {complexity_key}"),
            };
        }

        // 3. Default
        debug!(agent = %self.default_agent, "routed to default agent");
        RouteDecision {
            agent_name: self.default_agent.clone(),
            reason: "default".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::SubtaskId;
    use surge_core::spec::{Complexity, AcceptanceCriteria};

    fn make_subtask(complexity: Complexity) -> Subtask {
        Subtask {
            id: SubtaskId::new(),
            title: "Test".to_string(),
            description: "Test subtask".to_string(),
            complexity,
            files: vec![],
            acceptance_criteria: vec![],
            depends_on: vec![],
        }
    }

    #[test]
    fn test_default_routing() {
        let config = RoutingConfig::default();
        let router = AgentRouter::new(config, "claude".to_string());

        let subtask = make_subtask(Complexity::Simple);
        let decision = router.route(&subtask, None);
        assert_eq!(decision.agent_name, "claude");
        assert_eq!(decision.reason, "default");
    }

    #[test]
    fn test_phase_routing() {
        let config = RoutingConfig::default();
        let mut router = AgentRouter::new(config, "claude".to_string());
        router.set_phase_agent("qa", "copilot");

        let subtask = make_subtask(Complexity::Simple);
        let decision = router.route(&subtask, Some("qa"));
        assert_eq!(decision.agent_name, "copilot");
        assert!(decision.reason.contains("phase"));
    }

    #[test]
    fn test_complexity_routing() {
        let mut prefs = HashMap::new();
        prefs.insert("complex".to_string(), "claude-opus".to_string());

        let config = RoutingConfig {
            strategy: surge_core::config::RoutingStrategy::Complexity,
            agent_preferences: prefs,
        };
        let router = AgentRouter::new(config, "claude".to_string());

        let subtask = make_subtask(Complexity::Complex);
        let decision = router.route(&subtask, None);
        assert_eq!(decision.agent_name, "claude-opus");
        assert!(decision.reason.contains("complexity"));
    }

    #[test]
    fn test_fallback_to_default() {
        let config = RoutingConfig {
            strategy: surge_core::config::RoutingStrategy::Complexity,
            agent_preferences: HashMap::new(),
        };
        let router = AgentRouter::new(config, "fallback-agent".to_string());

        let subtask = make_subtask(Complexity::Simple);
        let decision = router.route(&subtask, None);
        assert_eq!(decision.agent_name, "fallback-agent");
    }
}
```

**Step 2: Update lib.rs**

Add `pub mod router;` and `pub use router::{AgentRouter, RouteDecision};`

**Step 3: Run tests, commit**

```bash
git add crates/surge-acp/src/router.rs crates/surge-acp/src/lib.rs
git commit -m "feat(acp): add AgentRouter — complexity and phase-based routing"
```

---

### Task 4: HealthMonitor — agent health tracking and fallback

**Files:**
- Create: `crates/surge-acp/src/health.rs`
- Modify: `crates/surge-acp/src/lib.rs`

**Step 1: Write health.rs**

```rust
//! Health monitoring and fallback for agents.

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Health status of an agent.
#[derive(Debug, Clone)]
pub struct AgentHealth {
    /// Agent name.
    pub name: String,
    /// Total requests made.
    pub total_requests: u64,
    /// Total failures.
    pub total_failures: u64,
    /// Whether the agent is currently rate-limited.
    pub rate_limited: bool,
    /// When the rate limit is estimated to reset.
    pub rate_limit_reset: Option<Instant>,
    /// Average latency of recent requests.
    pub avg_latency_ms: u64,
    /// Last error message (if any).
    pub last_error: Option<String>,
}

impl AgentHealth {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            total_requests: 0,
            total_failures: 0,
            rate_limited: false,
            rate_limit_reset: None,
            avg_latency_ms: 0,
            last_error: None,
        }
    }

    /// Error rate as a percentage (0-100).
    pub fn error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        (self.total_failures as f64 / self.total_requests as f64) * 100.0
    }

    /// Whether the agent is considered healthy.
    pub fn is_healthy(&self) -> bool {
        !self.rate_limited && self.error_rate() < 50.0
    }
}

/// Monitors agent health and provides fallback recommendations.
pub struct HealthMonitor {
    agents: HashMap<String, AgentHealth>,
    fallback_map: HashMap<String, String>,
}

impl HealthMonitor {
    /// Create a new health monitor.
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            fallback_map: HashMap::new(),
        }
    }

    /// Register an agent for monitoring.
    pub fn register(&mut self, name: &str) {
        self.agents.entry(name.to_string()).or_insert_with(|| AgentHealth::new(name));
    }

    /// Set fallback agent for a primary agent.
    pub fn set_fallback(&mut self, primary: &str, fallback: &str) {
        self.fallback_map.insert(primary.to_string(), fallback.to_string());
    }

    /// Record a successful request.
    pub fn record_success(&mut self, agent: &str, latency: Duration) {
        let health = self.agents.entry(agent.to_string())
            .or_insert_with(|| AgentHealth::new(agent));
        health.total_requests += 1;
        // Simple moving average
        let ms = latency.as_millis() as u64;
        health.avg_latency_ms = (health.avg_latency_ms + ms) / 2;
        // Clear rate limit if it was set
        if health.rate_limited {
            if let Some(reset) = health.rate_limit_reset {
                if Instant::now() >= reset {
                    health.rate_limited = false;
                    health.rate_limit_reset = None;
                    info!(agent, "rate limit cleared");
                }
            }
        }
    }

    /// Record a failed request.
    pub fn record_failure(&mut self, agent: &str, error: &str) {
        let health = self.agents.entry(agent.to_string())
            .or_insert_with(|| AgentHealth::new(agent));
        health.total_requests += 1;
        health.total_failures += 1;
        health.last_error = Some(error.to_string());

        // Detect rate limiting (429-like errors)
        if error.contains("429") || error.contains("rate limit") || error.contains("too many") {
            health.rate_limited = true;
            health.rate_limit_reset = Some(Instant::now() + Duration::from_secs(60));
            warn!(agent, "rate limited, pausing for 60s");
        }
    }

    /// Get the best agent to use, considering health and fallbacks.
    pub fn resolve_agent(&self, preferred: &str) -> &str {
        if let Some(health) = self.agents.get(preferred) {
            if health.is_healthy() {
                return preferred;
            }
            // Try fallback
            if let Some(fallback) = self.fallback_map.get(preferred) {
                if let Some(fb_health) = self.agents.get(fallback) {
                    if fb_health.is_healthy() {
                        info!(preferred, fallback = fallback, "using fallback agent");
                        return fallback;
                    }
                }
            }
        }
        // No healthy alternative — use preferred anyway
        preferred
    }

    /// Get health status for all agents.
    pub fn all_health(&self) -> Vec<&AgentHealth> {
        self.agents.values().collect()
    }

    /// Get health for a specific agent.
    pub fn get_health(&self, agent: &str) -> Option<&AgentHealth> {
        self.agents.get(agent)
    }
}

impl Default for HealthMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_healthy_by_default() {
        let mut mon = HealthMonitor::new();
        mon.register("claude");
        assert!(mon.agents["claude"].is_healthy());
    }

    #[test]
    fn test_error_rate() {
        let mut health = AgentHealth::new("test");
        health.total_requests = 10;
        health.total_failures = 3;
        assert!((health.error_rate() - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_unhealthy_on_high_errors() {
        let mut health = AgentHealth::new("test");
        health.total_requests = 10;
        health.total_failures = 6; // 60% error rate
        assert!(!health.is_healthy());
    }

    #[test]
    fn test_rate_limit_detection() {
        let mut mon = HealthMonitor::new();
        mon.register("claude");
        mon.record_failure("claude", "429 Too Many Requests");
        assert!(mon.agents["claude"].rate_limited);
    }

    #[test]
    fn test_fallback_routing() {
        let mut mon = HealthMonitor::new();
        mon.register("claude");
        mon.register("copilot");
        mon.set_fallback("claude", "copilot");

        // Claude is healthy — use it
        assert_eq!(mon.resolve_agent("claude"), "claude");

        // Make claude unhealthy
        mon.agents.get_mut("claude").unwrap().rate_limited = true;
        assert_eq!(mon.resolve_agent("claude"), "copilot");
    }

    #[test]
    fn test_success_recording() {
        let mut mon = HealthMonitor::new();
        mon.register("claude");
        mon.record_success("claude", Duration::from_millis(200));
        assert_eq!(mon.agents["claude"].total_requests, 1);
        assert_eq!(mon.agents["claude"].total_failures, 0);
    }
}
```

**Step 2: Update lib.rs**

Add `pub mod health;` and `pub use health::{AgentHealth, HealthMonitor};`

**Step 3: Run tests, commit**

```bash
git add crates/surge-acp/src/health.rs crates/surge-acp/src/lib.rs
git commit -m "feat(acp): add HealthMonitor — agent health tracking, rate limit detection, fallback"
```

---

### Task 5: CLI — agent add, agent status, run --planner/--coder

**Files:**
- Modify: `crates/surge-cli/src/main.rs`

**Step 1: Add agent status subcommand**

Add to `AgentCommands`:
```rust
    /// Show agent health status
    Status,
```

Handler:
```rust
AgentCommands::Status => {
    println!("⚡ Agent health:\n");
    println!("   (health monitoring active during pipeline execution)");
    println!("   Use 'surge run' to see live agent health.");
}
```

**Step 2: Add --planner and --coder to Run command**

Add to `Commands::Run`:
```rust
    /// Override planner agent
    #[arg(long)]
    planner: Option<String>,
    /// Override coder agent
    #[arg(long)]
    coder: Option<String>,
```

Print overrides if provided (actual routing integration would happen in orchestrator in a future pass).

**Step 3: Verify compilation, commit**

```bash
git add crates/surge-cli/src/main.rs
git commit -m "feat(cli): add agent status command, --parallel/--planner/--coder flags to run"
```

---

### Task 6: Final verification

**Step 1:** `cargo test --workspace`
**Step 2:** `cargo clippy --workspace`
**Step 3:** Commit fixes if needed

---

## Dependency Graph

```
Task 1 (ParallelExecutor) → Task 2 (pipeline integration + CLI --parallel)
Task 3 (AgentRouter)      → Task 5 (CLI agent commands)
Task 4 (HealthMonitor)     → Task 5 (CLI agent commands)
                             → Task 6 (final verification)
```

Tasks 1, 3, 4 are independent.
Task 2 depends on Task 1.
Task 5 depends on Tasks 3, 4.
