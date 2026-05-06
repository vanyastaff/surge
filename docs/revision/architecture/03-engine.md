# Architecture 03 · Engine

## Overview

The engine is the heart of surge. It consumes events, advances run state, executes nodes, and produces new events. This document specifies the engine's internal structure, the executor's main loop, scheduling, and error recovery.

This is implementation-level detail building on RFC-0002 (execution model).

## Module structure

```
crates/engine/src/
├── lib.rs
├── executor/
│   ├── mod.rs              (Executor entry point + main loop)
│   ├── stage.rs            (per-NodeKind stage execution)
│   ├── routing.rs          (outcome → edge → next node)
│   └── retry.rs            (retry logic, backoff)
├── scheduler/
│   ├── mod.rs              (multi-run scheduling)
│   └── daemon.rs           (daemon process management)
├── bootstrap/
│   ├── mod.rs              (bootstrap orchestration)
│   ├── description.rs      (Description Author wrapper)
│   ├── roadmap.rs          (Roadmap Planner wrapper)
│   └── flow.rs             (Flow Generator wrapper)
├── hooks/
│   ├── mod.rs              (hook execution)
│   └── matcher.rs          (predicate matching)
├── recovery/
│   ├── mod.rs              (crash recovery)
│   └── replay_to_state.rs  (fold events to current state)
├── memory.rs               (RunMemory accumulator)
├── error.rs
└── tests/
```

## Executor

The executor is the engine's main loop for a single run. It consumes events from the run's event log and produces new events.

### Lifecycle

```rust
pub struct Executor {
    run_id: RunId,
    storage: Arc<Storage>,
    acp_bridge: Arc<AcpBridge>,
    state: RunState,
    config: RunConfig,
}

impl Executor {
    pub async fn new(run_id: RunId, ...) -> Result<Self> { ... }
    
    pub async fn run(mut self) -> Result<TerminalKind> {
        loop {
            // Re-fold events from current seq cursor to head
            self.refresh_state().await?;
            
            match &self.state {
                RunState::NotStarted => self.start_run().await?,
                RunState::Bootstrapping { .. } => self.advance_bootstrap().await?,
                RunState::Pipeline { .. } => self.advance_pipeline().await?,
                RunState::Terminal { kind, .. } => return Ok(*kind),
            }
        }
    }
    
    async fn refresh_state(&mut self) -> Result<()> { ... }
    async fn advance_pipeline(&mut self) -> Result<()> {
        let cursor = self.state.cursor()?;
        let node = self.state.graph()?.nodes.get(&cursor.node)?;
        
        match node.kind {
            NodeKind::Agent => self.execute_agent_stage(node).await,
            NodeKind::HumanGate => self.execute_human_gate(node).await,
            NodeKind::Branch => self.execute_branch(node).await,
            NodeKind::Terminal => self.execute_terminal(node).await,
            NodeKind::Notify => self.execute_notify(node).await,
            NodeKind::Loop => self.execute_loop(node).await,
            NodeKind::Subgraph => self.execute_subgraph(node).await,
        }
    }
}
```

### Per-NodeKind execution

#### Agent

```rust
async fn execute_agent_stage(&mut self, node: &Node) -> Result<()> {
    let cfg = node.config.as_agent()?;
    let attempt = self.next_attempt_for(&node.id);
    
    self.write_event(EventPayload::StageEntered {
        node: node.id.clone(),
        attempt,
    }).await?;
    
    // 1. Resolve bindings
    let bindings = self.resolve_bindings(&cfg.bindings).await?;
    self.write_event(EventPayload::StageInputsResolved {
        node: node.id.clone(),
        bindings: bindings.clone(),
    }).await?;
    
    // 2. Build agent invocation context
    let profile = self.load_profile(&cfg.profile).await?;
    let prompt = self.render_prompt(&profile, &bindings, cfg.prompt_overrides.as_ref())?;
    let launch_cfg = self.compute_agent_launch_config(&profile, cfg.launch_override.as_ref())?;
    let sandbox_cfg = self.compute_agent_sandbox_intent(&profile, cfg.sandbox_override.as_ref())?;
    let tools = self.compute_tools(&profile, cfg.tool_overrides.as_ref(), &sandbox_cfg)?;
    
    // 3. Open ACP session
    let session = self.acp_bridge.open_session(SessionConfig {
        launch: launch_cfg.clone(),
        model: profile.runtime.recommended_model.clone(),
        system_prompt: prompt,
        tools: tools.clone(),
        sandbox: sandbox_cfg.clone(),
        run_dir: self.run_dir(),
    }).await?;
    
    self.write_event(EventPayload::SessionOpened {
        node: node.id.clone(),
        session: session.id.clone(),
        agent: launch_cfg.provider.clone(),
        launch_mode: launch_cfg.mode.clone(),
        sandbox_mode: sandbox_cfg.mode.clone(),
    }).await?;
    
    // 4. Run pre_tool_use hooks (none yet, will be triggered per-tool by ACP bridge)
    
    // 5. Drive the session, observe events
    let outcome = self.drive_session(&session, &cfg.limits, &node.id).await?;
    
    // 6. Run on_outcome hooks
    let hook_result = self.run_hooks(HookTrigger::OnOutcome, node, &outcome).await?;
    if hook_result.rejected {
        self.write_event(EventPayload::OutcomeRejectedByHook {
            node: node.id.clone(),
            outcome: outcome.clone(),
            hook_id: hook_result.hook_id,
        }).await?;
        // Retry stage if attempts available
        if attempt < cfg.limits.max_retries {
            return Ok(()); // loop will re-enter with attempt+1
        } else {
            self.write_event(EventPayload::StageFailed {
                node: node.id.clone(),
                reason: "Hook rejection exhausted retries".to_string(),
                retry_available: false,
            }).await?;
            return Err(EngineError::HookExhaustion);
        }
    }
    
    // 7. Validate outcome is declared
    if !node.declared_outcomes.iter().any(|o| o.id == outcome) {
        return Err(EngineError::UndeclaredOutcome { reported: outcome });
    }
    
    self.write_event(EventPayload::StageCompleted {
        node: node.id.clone(),
        outcome: outcome.clone(),
    }).await?;
    
    // 8. Close session
    self.acp_bridge.close_session(&session.id).await?;
    self.write_event(EventPayload::SessionClosed {
        session: session.id.clone(),
        disposition: SessionDisposition::Normal,
    }).await?;
    
    // 9. Route to next node
    self.route_outcome(&node.id, &outcome).await?;
    
    Ok(())
}
```

#### HumanGate

```rust
async fn execute_human_gate(&mut self, node: &Node) -> Result<()> {
    let cfg = node.config.as_human_gate()?;
    
    self.write_event(EventPayload::StageEntered {
        node: node.id.clone(),
        attempt: 1, // gates don't retry
    }).await?;
    
    // Render summary
    let bindings = self.resolve_bindings_for_summary(&cfg.summary).await?;
    let payload = self.render_summary(&cfg.summary, &bindings)?;
    
    // Send to first available channel
    for channel in &cfg.channels {
        match self.send_approval_request(channel, &node.id, &payload).await {
            Ok(()) => break,
            Err(e) if matches!(e, ChannelError::Unavailable) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    
    self.write_event(EventPayload::ApprovalRequested {
        gate: node.id.clone(),
        channel: cfg.channels[0].clone(),  // primary
        payload_hash: hash(&payload),
    }).await?;
    
    // Suspend execution; engine subprocess persists, but this run waits
    let decision = self.wait_for_approval(&node.id, cfg.timeout_seconds).await?;
    
    self.write_event(EventPayload::ApprovalDecided {
        gate: node.id.clone(),
        decision: decision.outcome.clone(),
        channel: decision.channel,
        comment: decision.comment,
    }).await?;
    
    self.write_event(EventPayload::StageCompleted {
        node: node.id.clone(),
        outcome: decision.outcome.clone(),
    }).await?;
    
    self.route_outcome(&node.id, &decision.outcome).await?;
    Ok(())
}
```

#### Branch

```rust
async fn execute_branch(&mut self, node: &Node) -> Result<()> {
    let cfg = node.config.as_branch()?;
    
    self.write_event(EventPayload::StageEntered { node: node.id.clone(), attempt: 1 }).await?;
    
    let memory = self.state.pipeline_memory()?;
    let outcome = self.evaluate_branch(&cfg, memory)?;
    
    self.write_event(EventPayload::StageCompleted {
        node: node.id.clone(),
        outcome: outcome.clone(),
    }).await?;
    
    self.route_outcome(&node.id, &outcome).await?;
    Ok(())
}

fn evaluate_branch(&self, cfg: &BranchConfig, memory: &RunMemory) -> Result<OutcomeId> {
    for arm in &cfg.predicates {
        if self.evaluate_predicate(&arm.condition, memory)? {
            return Ok(arm.outcome.clone());
        }
    }
    Ok(cfg.default_outcome.clone())
}
```

#### Terminal

```rust
async fn execute_terminal(&mut self, node: &Node) -> Result<()> {
    let cfg = node.config.as_terminal()?;
    
    let event = match cfg.kind {
        TerminalKind::Success => EventPayload::RunCompleted { terminal: node.id.clone() },
        TerminalKind::Failure { exit_code } => EventPayload::RunFailed {
            error: format!("Terminal::Failure with exit code {}", exit_code),
        },
        TerminalKind::Aborted => EventPayload::RunAborted {
            reason: cfg.message.clone().unwrap_or_default(),
        },
    };
    
    self.write_event(event).await?;
    self.send_termination_notification(&cfg).await?;
    Ok(())
}
```

#### Loop

```rust
async fn execute_loop(&mut self, node: &Node) -> Result<()> {
    let cfg = node.config.as_loop()?;
    
    self.write_event(EventPayload::StageEntered { node: node.id.clone(), attempt: 1 }).await?;
    
    let items = self.resolve_iterable(&cfg.iterates_over).await?;
    let body_graph = &cfg.body;
    
    let mut completed_iterations = 0;
    let mut last_outcome = OutcomeId::from("completed");
    
    for (idx, item) in items.iter().enumerate() {
        self.write_event(EventPayload::LoopIterationStarted {
            loop_id: node.id.clone(),
            item: item.clone(),
            index: idx as u32,
        }).await?;
        
        // Execute body subgraph with item bound to iteration_var
        let iter_outcome = self.execute_subgraph_with_context(
            body_graph,
            &cfg.iteration_var_name,
            item,
        ).await;
        
        match iter_outcome {
            Ok(outcome) => {
                self.write_event(EventPayload::LoopIterationCompleted {
                    loop_id: node.id.clone(),
                    index: idx as u32,
                    outcome: outcome.clone(),
                }).await?;
                completed_iterations += 1;
                last_outcome = outcome;
                
                // Check exit condition
                if self.check_exit_condition(&cfg.exit_condition, &last_outcome, completed_iterations) {
                    break;
                }
                
                // Optional gate between iterations
                if cfg.gate_after_each {
                    let gate_decision = self.run_inter_iteration_gate(&node.id, idx).await?;
                    if gate_decision == "abort" { break; }
                }
            }
            Err(e) => {
                match cfg.on_iteration_failure {
                    FailurePolicy::Abort => return Err(e),
                    FailurePolicy::Skip => continue,
                    FailurePolicy::Retry { max } => {
                        if let Ok(o) = self.retry_iteration(body_graph, item, max).await {
                            last_outcome = o;
                            completed_iterations += 1;
                        }
                    }
                    FailurePolicy::Replan => {
                        // Implementation-specific: signal back to outer planner
                        return Err(EngineError::ReplanRequested);
                    }
                }
            }
        }
    }
    
    self.write_event(EventPayload::LoopCompleted {
        loop_id: node.id.clone(),
        completed_iterations,
        final_outcome: last_outcome.clone(),
    }).await?;
    
    self.write_event(EventPayload::StageCompleted {
        node: node.id.clone(),
        outcome: last_outcome.clone(),
    }).await?;
    
    self.route_outcome(&node.id, &last_outcome).await?;
    Ok(())
}
```

### Routing

```rust
async fn route_outcome(&mut self, node: &NodeId, outcome: &OutcomeId) -> Result<()> {
    let graph = self.state.graph()?;
    let port = PortRef { node: node.clone(), outcome: outcome.clone() };
    
    let edge = graph.edges.iter().find(|e| e.from == port)
        .ok_or(EngineError::UndeclaredOutcomeRoute { node: node.clone(), outcome: outcome.clone() })?;
    
    // Check max_traversals for cycles
    if let Some(max) = edge.policy.max_traversals {
        let traversal_count = self.count_edge_traversals(&edge.id);
        if traversal_count >= max {
            match edge.policy.on_max_exceeded {
                ExceededAction::Escalate => {
                    self.escalate_max_traversals(&edge.id, traversal_count).await?;
                    return Ok(());
                }
                ExceededAction::Fail => {
                    return Err(EngineError::MaxTraversalsExceeded);
                }
            }
        }
    }
    
    self.write_event(EventPayload::EdgeTraversed {
        edge: edge.id.clone(),
        from: node.clone(),
        to: edge.to.clone(),
    }).await?;
    
    Ok(())
}
```

### Retry logic

```rust
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff: BackoffStrategy,
    pub jitter: bool,
}

pub enum BackoffStrategy {
    Constant { delay: Duration },
    Linear { base: Duration },
    Exponential { base: Duration, max: Duration },
}

impl Executor {
    fn next_attempt_for(&self, node: &NodeId) -> u32 {
        // Count StageEntered events for this node
        self.state.events_for_node(node)
            .iter()
            .filter(|e| matches!(e.payload, EventPayload::StageEntered { .. }))
            .count() as u32 + 1
    }
}
```

Retries are implicit in the executor's main loop: after `StageFailed`, next iteration of `advance_pipeline` re-enters the same node with incremented attempt counter.

## Scheduler

For multiple concurrent runs, the scheduler manages daemon processes.

```rust
pub struct Scheduler {
    runs: Arc<RwLock<HashMap<RunId, RunHandle>>>,
    storage: Arc<Storage>,
}

pub struct RunHandle {
    pub run_id: RunId,
    pub daemon_pid: Option<u32>,
    pub status: RunStatus,
}

impl Scheduler {
    pub async fn start_run(&self, run_id: RunId, config: RunConfig) -> Result<()> {
        // Spawn daemon subprocess
        let pid = spawn_daemon(run_id.clone(), config).await?;
        self.storage.update_run_daemon_pid(&run_id, pid).await?;
        
        let handle = RunHandle {
            run_id: run_id.clone(),
            daemon_pid: Some(pid),
            status: RunStatus::Bootstrapping,
        };
        self.runs.write().await.insert(run_id, handle);
        Ok(())
    }
    
    pub async fn cancel_run(&self, run_id: &RunId) -> Result<()> {
        let handle = self.runs.read().await.get(run_id).cloned();
        if let Some(h) = handle {
            if let Some(pid) = h.daemon_pid {
                send_signal(pid, Signal::Term)?;
            }
            self.storage.append_event(run_id, EventPayload::RunAborted {
                reason: "User-initiated cancel".to_string(),
            }).await?;
        }
        Ok(())
    }
}
```

## Daemon process

A daemon is a separate process running the executor for a single run. The CLI spawns it and detaches.

```rust
// crates/engine/src/scheduler/daemon.rs

#[cfg(unix)]
pub async fn spawn_daemon(run_id: RunId, config: RunConfig) -> Result<u32> {
    use nix::unistd::{fork, ForkResult, setsid};
    
    match unsafe { fork() }? {
        ForkResult::Parent { child } => {
            // Parent records PID and returns
            Ok(child.as_raw() as u32)
        }
        ForkResult::Child => {
            setsid()?;  // detach from terminal
            // Redirect stdin/stdout/stderr
            // Run the executor
            let executor = Executor::new(run_id, ...).await?;
            executor.run().await?;
            std::process::exit(0);
        }
    }
}

#[cfg(windows)]
pub async fn spawn_daemon(run_id: RunId, config: RunConfig) -> Result<u32> {
    // Use CreateProcess with DETACHED_PROCESS flag
    use std::os::windows::process::CommandExt;
    
    let child = Command::new(std::env::current_exe()?)
        .arg("--daemon")
        .arg("--run-id").arg(run_id.to_string())
        .creation_flags(0x00000008)  // DETACHED_PROCESS
        .spawn()?;
    
    Ok(child.id())
}
```

## Crash recovery

On engine startup (called from `surge doctor` or `surge attach`):

```rust
pub async fn recover_runs(storage: &Storage) -> Result<Vec<RecoveryAction>> {
    let runs = storage.list_runs_in_status(&[
        RunStatus::Bootstrapping,
        RunStatus::Running,
    ]).await?;
    
    let mut actions = Vec::new();
    
    for run in runs {
        // Check if daemon is alive
        let daemon_alive = run.daemon_pid
            .map(|pid| is_process_alive(pid))
            .unwrap_or(false);
        
        if daemon_alive {
            // Run is still going, leave alone
            actions.push(RecoveryAction::Skip { run_id: run.id });
            continue;
        }
        
        // Daemon died — check if recoverable
        let state = fold_run_to_state(&run.id, storage).await?;
        match state {
            RunState::Pipeline { cursor, .. } => {
                // We were in the middle of a pipeline node
                // Mark current attempt as failed, retry will happen on resume
                storage.append_event(&run.id, EventPayload::StageFailed {
                    node: cursor.node.clone(),
                    reason: "Daemon crashed".to_string(),
                    retry_available: true,
                }).await?;
                
                // Schedule for resume
                actions.push(RecoveryAction::Resume { run_id: run.id });
            }
            RunState::Bootstrapping { substate: BootstrapSubstate::AwaitingApproval { .. }, .. } => {
                // We're waiting on user; just need to ensure approval card was delivered
                actions.push(RecoveryAction::ResendPendingApprovals { run_id: run.id });
            }
            _ => {
                actions.push(RecoveryAction::Skip { run_id: run.id });
            }
        }
    }
    
    Ok(actions)
}
```

## Hook execution

```rust
pub struct HookExecutor {
    project_root: PathBuf,
}

impl HookExecutor {
    pub async fn execute(&self, hook: &Hook, ctx: &HookContext) -> Result<HookResult> {
        let cmd_with_vars = self.expand_vars(&hook.command, ctx);
        
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd_with_vars)
            .current_dir(&self.project_root)
            .output()
            .timeout(Duration::from_secs(hook.timeout_seconds.unwrap_or(30)))
            .await??;
        
        let result = HookResult {
            hook_id: hook.id.clone(),
            exit_status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into(),
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        };
        
        match (result.exit_status, &hook.on_failure) {
            (0, _) => Ok(result),
            (_, HookFailureMode::Reject) => {
                Ok(result)  // caller checks exit_status and handles rejection
            }
            (_, HookFailureMode::Warn) => {
                tracing::warn!(hook = %hook.id, exit = result.exit_status, "Hook failed (warn-only)");
                Ok(HookResult { exit_status: 0, ..result })  // mask as success
            }
            (_, HookFailureMode::Ignore) => {
                Ok(HookResult { exit_status: 0, ..result })
            }
        }
    }
}
```

## RunMemory

Accumulator of derived state from events. Used for branch predicates, summary rendering, etc.

```rust
pub struct RunMemory {
    pub artifacts: HashMap<String, ArtifactRef>,        // by name
    pub artifacts_by_node: HashMap<NodeId, Vec<ArtifactRef>>,
    pub outcomes: HashMap<NodeId, Vec<OutcomeRecord>>,  // history per node
    pub costs: CostSummary,
    pub trust_state: TrustState,
}

impl RunMemory {
    pub fn from_events(events: &[Event]) -> Self {
        let mut memory = Self::default();
        for event in events {
            memory.apply(event);
        }
        memory
    }
    
    fn apply(&mut self, event: &Event) {
        match &event.payload {
            EventPayload::ArtifactProduced { node, artifact_id, path } => {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                let aref = ArtifactRef { id: artifact_id.clone(), path: path.clone(), name: name.clone() };
                self.artifacts.insert(name, aref.clone());
                self.artifacts_by_node.entry(node.clone()).or_default().push(aref);
            }
            EventPayload::OutcomeReported { node, outcome, summary } => {
                self.outcomes.entry(node.clone()).or_default().push(OutcomeRecord {
                    outcome: outcome.clone(),
                    summary: summary.clone(),
                    seq: event.seq,
                });
            }
            EventPayload::TokensConsumed { prompt_tokens, output_tokens, .. } => {
                self.costs.tokens_in += *prompt_tokens as u64;
                self.costs.tokens_out += *output_tokens as u64;
            }
            // ... other event types
            _ => {}
        }
    }
}
```

## Concurrency model

- **Within a run**: single-threaded executor (one node at a time).
- **Across runs**: each run is a separate daemon process.
- **Event log writes**: only the run's own daemon writes. Other processes (CLI, UI) read.
- **SQLite WAL mode**: required for read-during-write.

## Error taxonomy

```rust
pub enum EngineError {
    // Input errors (invalid graph, etc.)
    InvalidGraph { reason: String },
    UndeclaredOutcome { reported: OutcomeId },
    UndeclaredOutcomeRoute { node: NodeId, outcome: OutcomeId },
    
    // Runtime errors
    StageTimeout { node: NodeId },
    HookExhaustion,
    SessionFailed(AcpError),
    ProviderLaunchUnsupported { provider: String, requested: String },
    ProviderSandboxUnsupported { provider: String, requested: String },
    PermissionDenied { capability: String },
    MaxTraversalsExceeded,
    ReplanRequested,
    
    // Storage errors
    StorageError(StorageError),
    
    // External errors
    ApprovalChannelError(ChannelError),
    
    // Unrecoverable
    DataCorruption { details: String },
}
```

Each variant has handling rules in the executor — some retry, some escalate to HumanGate, some fail the run.

## Acceptance criteria

The engine is correctly implemented when:

1. A simple linear flow (`Agent → Agent → Terminal`) runs end-to-end without errors.
2. A flow with HumanGate pauses correctly, persists state across crash, and resumes after approval.
3. A Loop node with 5 iterations executes the body 5 times with correct context binding.
4. A nested Loop (outer + inner) works, including failure handling per `on_iteration_failure` policy.
5. Hook rejections cause stage retry up to `max_retries`, then fail.
6. Crash recovery: SIGKILL the daemon mid-stage → restart engine → run resumes with attempt+1 marker.
7. Multiple concurrent runs (3+) execute without interference, each with its own daemon.
8. All event types specified in RFC-0002 are produced at appropriate moments.
9. Branch predicates evaluate correctly against `RunMemory` for all `Predicate` variants.
10. End-to-end test: full TDD pipeline (7 nodes) completes successfully on a fixture project.
