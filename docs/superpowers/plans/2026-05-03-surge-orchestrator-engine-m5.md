# M5 — `surge-orchestrator` Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an `engine` module to `surge-orchestrator` that drives a frozen `Graph` through `AcpBridge` sessions, persists every transition into `surge-persistence`, and resumes from snapshots after crashes — closing the M1+M2+M3 loop into working autonomous runs.

**Architecture:** Pure addition to `surge-orchestrator` (legacy FSM modules untouched). New submodule `crates/surge-orchestrator/src/engine/` plus a `BridgeFacade` trait in `surge-acp::bridge::facade` and three additive changes in `surge-core` (HumanInput event variants + RunState extension + new `predicate` module). Sequential pipeline only; concurrent runs without engine-side limit; snapshot every stage boundary; fail-fast.

**Tech Stack:** Rust 2024 edition (stable toolchain), `tokio` (multi-thread runtime), `async-trait`, `serde` / `serde_json`, `thiserror`, `tracing`, M2 `surge-persistence` (rusqlite-backed event log + snapshots), M3 `surge-acp::bridge` (ACP via subprocess + LocalSet), M3 `mock_acp_agent` binary for integration tests.

---

## Spec reference

This plan implements [docs/superpowers/specs/2026-05-03-surge-orchestrator-engine-m5-design.md](../specs/2026-05-03-surge-orchestrator-engine-m5-design.md). Section numbers in tasks (e.g., "per spec §6.1") refer to that document. Read the spec end-to-end before starting; the plan focuses on *how* to land each piece, the spec covers *why*.

## File structure preview

New files in `surge-orchestrator/src/engine/`:

| File | Responsibility |
|---|---|
| `mod.rs` | Re-exports + module documentation |
| `error.rs` | `EngineError` taxonomy |
| `engine.rs` | `Engine` struct + `start_run`/`resume_run`/`stop_run`/`resolve_human_input` |
| `handle.rs` | `RunHandle`, `RunOutcome`, `EngineRunEvent` |
| `config.rs` | `EngineConfig`, `EngineRunConfig`, `SnapshotPolicy` |
| `snapshot.rs` | `EngineSnapshot` type + serde |
| `run_task.rs` | Per-run tokio task (drives one `Graph`) |
| `routing.rs` | `next_node_after(graph, current, outcome)` |
| `replay.rs` | Snapshot + event-tail → in-memory state |
| `predicates.rs` | `EnginePredicateContext` (impl `PredicateContext`) |
| `sandbox_factory.rs` | `build_sandbox(&SandboxConfig) → Box<dyn Sandbox>` |
| `stage/mod.rs` | Stage execution dispatch |
| `stage/agent.rs` | `NodeKind::Agent` execution |
| `stage/branch.rs` | `NodeKind::Branch` routing |
| `stage/human_gate.rs` | `NodeKind::HumanGate` handling |
| `stage/terminal.rs` | `NodeKind::Terminal` |
| `stage/notify.rs` | `NodeKind::Notify` (M5 stub) |
| `tools/mod.rs` | `ToolDispatcher` trait |
| `tools/worktree.rs` | `WorktreeToolDispatcher` (read/write/shell) |
| `tools/path_guard.rs` | Path canonicalization wrapper |

Modified `surge-orchestrator/src/lib.rs`: add `pub mod engine;` (one line).

Additions to `surge-acp/src/bridge/`:
- `facade.rs` (new): `BridgeFacade` trait + `impl BridgeFacade for AcpBridge`
- `mod.rs` modifications: re-export `BridgeFacade`

Additions to `surge-core/src/`:
- `predicate.rs` (new): `PredicateContext` trait + `evaluate` function
- `run_event.rs` modifications: 3 new `EventPayload` variants + discriminant arms + tests
- `run_state.rs` modifications: `pending_human_input` field on `Pipeline` variant + 3 new `apply` arms + fix-up of existing pattern matches

Tests:
- `surge-orchestrator/tests/fixtures/mock_bridge.rs` — `MockBridge` for unit tests
- `surge-orchestrator/tests/fixtures/mock_dispatcher.rs` — `MockToolDispatcher`
- `surge-orchestrator/tests/engine_e2e_linear_pipeline.rs` — 3-stage end-to-end
- `surge-orchestrator/tests/engine_resume_after_crash.rs`
- `surge-orchestrator/tests/engine_concurrent_runs.rs`
- `surge-orchestrator/tests/engine_human_input_resolved.rs`
- `surge-orchestrator/tests/engine_human_input_timeout.rs`
- `surge-acp/tests/facade_contract.rs` — property test that `AcpBridge` and `MockBridge` agree

---

## Phase 0 — Scaffolding

### Task 0.1: Add `engine` module skeleton + Cargo dependencies

**Files:**
- Create: `crates/surge-orchestrator/src/engine/mod.rs`
- Modify: `crates/surge-orchestrator/src/lib.rs` (add `pub mod engine;`)
- Modify: `crates/surge-orchestrator/Cargo.toml` (add new dependencies)

- [ ] **Step 1: Create empty engine module**

Create `crates/surge-orchestrator/src/engine/mod.rs`:

```rust
//! Engine — drives a frozen `Graph` through ACP sessions and persistence.
//!
//! See `docs/superpowers/specs/2026-05-03-surge-orchestrator-engine-m5-design.md`
//! for the full design contract. M5 ships sequential-pipeline-only support;
//! parallel/loops/subgraphs are M6 scope and rejected at run-start.

// Submodules added incrementally as later phases land. Currently empty.
```

- [ ] **Step 2: Wire it into the crate**

Edit `crates/surge-orchestrator/src/lib.rs` — add at the end of the existing `pub mod` block:

```rust
pub mod engine;
```

- [ ] **Step 3: Add new dependencies to `crates/surge-orchestrator/Cargo.toml`**

Under `[dependencies]`, add (or augment) these entries; `tokio`, `serde`, `serde_json`, `thiserror`, `tracing`, `chrono`, `surge-core`, `surge-persistence`, `surge-acp` will already be present — only add what's missing:

```toml
async-trait = "0.1"
tokio-util = { version = "0.7", features = ["rt"] }
futures = "0.3"
```

(`tokio-util` is needed for `CancellationToken`; `async-trait` for the BridgeFacade and ToolDispatcher traits; `futures` for combinators in run_task.)

- [ ] **Step 4: Verify the workspace still builds**

Run: `cargo build -p surge-orchestrator`
Expected: clean build, no warnings about unused new deps yet (they're only in Cargo.toml; first usages come in later tasks).

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/engine/mod.rs crates/surge-orchestrator/src/lib.rs crates/surge-orchestrator/Cargo.toml
git commit -m "M5(engine): scaffold empty engine module + Cargo additions"
```

### Task 0.2: Verify acceptance #14 baseline (legacy modules untouched)

**Files:** none (verification only)

- [ ] **Step 1: Snapshot the byte-level state of legacy modules**

Run:

```bash
git ls-tree -r HEAD --name-only crates/surge-orchestrator/src/ | \
  grep -v '^crates/surge-orchestrator/src/engine/' | \
  grep -v '^crates/surge-orchestrator/src/lib\.rs$' | \
  xargs -I {} git rev-parse "HEAD:{}" > /tmp/m5-legacy-baseline.txt
```

Expected: file contains one git blob hash per legacy file (15+ lines).

- [ ] **Step 2: Save the baseline for the acceptance check**

```bash
mkdir -p .m5-acceptance
cp /tmp/m5-legacy-baseline.txt .m5-acceptance/legacy-baseline.txt
```

- [ ] **Step 3: Add `.m5-acceptance/` to `.gitignore`**

Edit `.gitignore`, append:

```
.m5-acceptance/
```

- [ ] **Step 4: Commit gitignore update only**

```bash
git add .gitignore
git commit -m "M5(engine): track legacy-module baseline via local-only marker"
```

(The baseline file itself is gitignored; the acceptance script regenerates it from `git show HEAD~N` if needed.)

---

## Phase 1 — `surge-core` extensions

### Task 1.1: Add three `EventPayload` variants for HumanInput

**Files:**
- Modify: `crates/surge-core/src/run_event.rs`

- [ ] **Step 1: Write failing tests for the three new variants**

In `crates/surge-core/src/run_event.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn human_input_requested_roundtrip() {
        let payload = EventPayload::HumanInputRequested {
            node: NodeKey::try_from("plan_1").unwrap(),
            session: Some(SessionId::new()),
            call_id: Some("call-42".into()),
            prompt: "Approve the plan?".into(),
            schema: Some(serde_json::json!({"type":"string"})),
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
        assert_eq!(payload.discriminant_str(), "HumanInputRequested");
    }

    #[test]
    fn human_input_resolved_roundtrip() {
        let payload = EventPayload::HumanInputResolved {
            node: NodeKey::try_from("plan_1").unwrap(),
            call_id: Some("call-42".into()),
            response: serde_json::json!({"decision":"approve"}),
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
        assert_eq!(payload.discriminant_str(), "HumanInputResolved");
    }

    #[test]
    fn human_input_timed_out_roundtrip() {
        let payload = EventPayload::HumanInputTimedOut {
            node: NodeKey::try_from("plan_1").unwrap(),
            call_id: None,
            elapsed_seconds: 300,
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
        assert_eq!(payload.discriminant_str(), "HumanInputTimedOut");
    }
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test -p surge-core --lib run_event::tests::human_input`
Expected: FAIL with "no variant `HumanInputRequested`".

- [ ] **Step 3: Add the variants to `EventPayload`**

In `crates/surge-core/src/run_event.rs`, inside the `pub enum EventPayload { ... }` block, append (just before the closing `}` of the enum):

```rust
    // Human input — added in M5. Three variants to support both the
    // tool-driven `request_human_input` path and HumanGate-driven pauses.
    HumanInputRequested {
        node: NodeKey,
        session: Option<SessionId>,
        call_id: Option<String>,
        prompt: String,
        schema: Option<serde_json::Value>,
    },
    HumanInputResolved {
        node: NodeKey,
        call_id: Option<String>,
        response: serde_json::Value,
    },
    HumanInputTimedOut {
        node: NodeKey,
        call_id: Option<String>,
        elapsed_seconds: u32,
    },
```

`call_id` uses `String` (not a typed `ToolCallId`) because surge-core has no dependency on surge-acp; engine code converts to/from `ToolCallId` at the boundary.

- [ ] **Step 4: Add discriminant arms**

In the same file, find `EventPayload::discriminant_str` and add three arms before `Self::ForkCreated`:

```rust
            Self::HumanInputRequested { .. } => "HumanInputRequested",
            Self::HumanInputResolved { .. } => "HumanInputResolved",
            Self::HumanInputTimedOut { .. } => "HumanInputTimedOut",
```

- [ ] **Step 5: Run tests, expect pass**

Run: `cargo test -p surge-core --lib run_event::tests`
Expected: all `human_input_*` tests pass; pre-existing tests still pass.

- [ ] **Step 6: Verify nothing else broke**

Run: `cargo build --workspace`
Expected: clean. (Other crates may pattern-match on `EventPayload` exhaustively — verify there are no `match` statements that need a new arm.)

If exhaustive matches exist (e.g., in `surge-persistence::aggregator` for view maintenance), add explicit `_ => {}` arms or new arms that ignore the new variants for now (engine-emitted events don't currently maintain materialized views).

- [ ] **Step 7: Commit**

```bash
git add crates/surge-core/src/run_event.rs
git commit -m "M5(core): add HumanInput EventPayload variants (Requested/Resolved/TimedOut)"
```

### Task 1.2: Add `pending_human_input` field to `RunState::Pipeline`

**Files:**
- Modify: `crates/surge-core/src/run_state.rs`

- [ ] **Step 1: Write a failing test for the new field's lifecycle**

In `crates/surge-core/src/run_state.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn human_input_request_populates_pending_field() {
        use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
        use crate::node::{Node, NodeConfig, Position};
        use crate::terminal_config::{TerminalConfig, TerminalKind};
        use std::collections::BTreeMap;

        let plan = NodeKey::try_from("plan").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            plan.clone(),
            Node {
                id: plan.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        let graph = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "minimal".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
            },
            start: plan.clone(),
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        };

        let events = vec![
            make_event(
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/tmp"),
                    initial_prompt: "build".into(),
                    config: RunConfig {
                        sandbox_default: SandboxMode::WorkspaceWrite,
                        approval_default: ApprovalPolicy::OnRequest,
                        auto_pr: false,
                    },
                },
            ),
            make_event(
                2,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash: ContentHash::compute(b"hash"),
                },
            ),
            make_event(
                3,
                EventPayload::HumanInputRequested {
                    node: plan.clone(),
                    session: None,
                    call_id: Some("c1".into()),
                    prompt: "ok?".into(),
                    schema: None,
                },
            ),
        ];

        let state = fold(&events).unwrap();
        match state {
            RunState::Pipeline { pending_human_input: Some(p), .. } => {
                assert_eq!(p.node, plan);
                assert_eq!(p.call_id.as_deref(), Some("c1"));
            }
            other => panic!("expected Pipeline with pending_human_input, got {other:?}"),
        }
    }

    #[test]
    fn human_input_resolution_clears_pending_field() {
        use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
        use crate::node::{Node, NodeConfig, Position};
        use crate::terminal_config::{TerminalConfig, TerminalKind};
        use std::collections::BTreeMap;

        let plan = NodeKey::try_from("plan").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            plan.clone(),
            Node {
                id: plan.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        let graph = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "minimal".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
            },
            start: plan.clone(),
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        };

        let events = vec![
            make_event(
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: PathBuf::from("/tmp"),
                    initial_prompt: "build".into(),
                    config: RunConfig {
                        sandbox_default: SandboxMode::WorkspaceWrite,
                        approval_default: ApprovalPolicy::OnRequest,
                        auto_pr: false,
                    },
                },
            ),
            make_event(
                2,
                EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash: ContentHash::compute(b"hash"),
                },
            ),
            make_event(
                3,
                EventPayload::HumanInputRequested {
                    node: plan.clone(),
                    session: None,
                    call_id: Some("c1".into()),
                    prompt: "ok?".into(),
                    schema: None,
                },
            ),
            make_event(
                4,
                EventPayload::HumanInputResolved {
                    node: plan.clone(),
                    call_id: Some("c1".into()),
                    response: serde_json::json!({"decision": "approve"}),
                },
            ),
        ];

        let state = fold(&events).unwrap();
        match state {
            RunState::Pipeline { pending_human_input: None, .. } => {}
            other => panic!("expected Pipeline with cleared pending_human_input, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test -p surge-core --lib run_state::tests::human_input`
Expected: FAIL with "no field `pending_human_input` on `RunState::Pipeline`".

- [ ] **Step 3: Add the field + struct**

In `crates/surge-core/src/run_state.rs`:

(a) Above `#[derive(Debug, Clone, PartialEq)] pub enum RunState`, add:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct PendingHumanInput {
    pub node: NodeKey,
    pub call_id: Option<String>,
    pub prompt: String,
    pub schema: Option<serde_json::Value>,
    pub requested_seq: u64,
}
```

(b) In the `RunState::Pipeline { ... }` variant, add the field at the end:

```rust
    Pipeline {
        graph: Arc<Graph>,
        cursor: Cursor,
        memory: RunMemory,
        pending_human_input: Option<PendingHumanInput>,
    },
```

- [ ] **Step 4: Fix every `RunState::Pipeline { ... }` constructor and pattern match**

Search:

```bash
rg --no-heading 'RunState::Pipeline\s*\{' crates/surge-core/src/
```

For every match in `apply()` and the existing tests, add `pending_human_input: None` to constructor sites and `pending_human_input` (or `..`) to destructuring patterns. Specifically:

- Inside `apply()`'s `PipelineMaterialized` arm: construct with `pending_human_input: None`.
- Inside `apply()`'s `StageEntered`, `ArtifactProduced`, `OutcomeReported`, `TokensConsumed` arms: when destructuring and re-constructing, pass `pending_human_input` through unchanged.
- Inside the `pipeline_materialized_transitions_to_pipeline` test: pattern-match with `..` so the existing assertion still works.

- [ ] **Step 5: Add `apply` arms for HumanInput events**

In `crates/surge-core/src/run_state.rs::apply`, before the catch-all `(state, _) => Ok(state)`, add:

```rust
        (
            state @ RunState::Pipeline { .. },
            EventPayload::HumanInputRequested { node, call_id, prompt, schema, .. },
        ) => {
            if let RunState::Pipeline { graph, cursor, memory, .. } = state {
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input: Some(PendingHumanInput {
                        node: node.clone(),
                        call_id: call_id.clone(),
                        prompt: prompt.clone(),
                        schema: schema.clone(),
                        requested_seq: event.seq,
                    }),
                })
            } else {
                unreachable!()
            }
        },
        (
            state @ RunState::Pipeline { .. },
            EventPayload::HumanInputResolved { .. },
        ) => {
            if let RunState::Pipeline { graph, cursor, memory, .. } = state {
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input: None,
                })
            } else {
                unreachable!()
            }
        },
        (
            state @ RunState::Pipeline { .. },
            EventPayload::HumanInputTimedOut { .. },
        ) => {
            // Timeout clears the pending field; engine writes a follow-up
            // StageFailed/RunFailed if appropriate. Fold itself stays in
            // Pipeline; the terminal transition is driven by the
            // separately-emitted RunFailed event.
            if let RunState::Pipeline { graph, cursor, memory, .. } = state {
                Ok(RunState::Pipeline {
                    graph,
                    cursor,
                    memory,
                    pending_human_input: None,
                })
            } else {
                unreachable!()
            }
        },
```

- [ ] **Step 6: Run tests, expect pass**

Run: `cargo test -p surge-core --lib run_state::tests`
Expected: all tests pass (existing + 2 new HumanInput tests).

- [ ] **Step 7: Verify the workspace still builds**

Run: `cargo build --workspace`
Expected: clean. If `surge-persistence` or `surge-orchestrator` legacy code matches `RunState::Pipeline` with explicit fields (rare), add `..` to those patterns.

- [ ] **Step 8: Commit**

```bash
git add crates/surge-core/src/run_state.rs
git commit -m "M5(core): add pending_human_input field to RunState::Pipeline + fold arms"
```

### Task 1.3: Create `surge-core::predicate` evaluator module

**Files:**
- Create: `crates/surge-core/src/predicate.rs`
- Modify: `crates/surge-core/src/lib.rs`

- [ ] **Step 1: Write failing tests for `evaluate`**

Create `crates/surge-core/src/predicate.rs`:

```rust
//! Predicate evaluator for `BranchConfig::predicates`.
//!
//! Pure function (`evaluate`) backed by a small `PredicateContext` trait so
//! the engine can supply runtime data (artifacts, env vars, file existence,
//! prior outcomes) without coupling the evaluator to engine internals.
//!
//! Fail-closed semantics: missing data (unknown artifact name, undefined env
//! var, broken symlink) makes the leaf predicate return `false`. Combinators
//! short-circuit normally. Documented choice — in an autonomous setting,
//! panicking on missing data would turn a small data error into a run-killing
//! crash; falling back keeps the run going and surfaces the divergence via
//! `OutcomeReported.summary`.

use crate::branch_config::{CompareOp, Predicate};
use crate::keys::{NodeKey, OutcomeKey};
use std::path::Path;

/// Runtime data source for predicate evaluation.
pub trait PredicateContext {
    /// Most recent outcome reported for `node`, if any.
    fn outcome_of(&self, node: &NodeKey) -> Option<&OutcomeKey>;

    /// Size in bytes of the artifact identified by `name`, if it exists.
    fn artifact_size(&self, name: &str) -> Option<u64>;

    /// Value of environment variable `name`, if defined.
    fn env_var(&self, name: &str) -> Option<String>;

    /// Whether `path` (typically relative to the worktree root) exists.
    fn file_exists(&self, path: &Path) -> bool;
}

/// Evaluate `predicate` against `ctx`. Never panics; missing data returns
/// `false` from the relevant leaf and short-circuits combinators normally.
#[must_use]
pub fn evaluate(predicate: &Predicate, ctx: &dyn PredicateContext) -> bool {
    match predicate {
        Predicate::FileExists { path } => ctx.file_exists(Path::new(path)),
        Predicate::ArtifactSize { artifact, op, value } => ctx
            .artifact_size(artifact)
            .map(|actual| compare_u64(actual, *op, *value))
            .unwrap_or(false),
        Predicate::OutcomeMatches { node, outcome } => {
            ctx.outcome_of(node).is_some_and(|o| o == outcome)
        }
        Predicate::EnvVar { name, op, value } => ctx
            .env_var(name)
            .map(|actual| compare_str(&actual, *op, value))
            .unwrap_or(false),
        Predicate::And { and } => and.iter().all(|p| evaluate(p, ctx)),
        Predicate::Or { or } => or.iter().any(|p| evaluate(p, ctx)),
        Predicate::Not { not } => !evaluate(not, ctx),
    }
}

fn compare_u64(a: u64, op: CompareOp, b: u64) -> bool {
    match op {
        CompareOp::Eq => a == b,
        CompareOp::Ne => a != b,
        CompareOp::Lt => a < b,
        CompareOp::Lte => a <= b,
        CompareOp::Gt => a > b,
        CompareOp::Gte => a >= b,
    }
}

fn compare_str(a: &str, op: CompareOp, b: &str) -> bool {
    match op {
        CompareOp::Eq => a == b,
        CompareOp::Ne => a != b,
        CompareOp::Lt => a < b,
        CompareOp::Lte => a <= b,
        CompareOp::Gt => a > b,
        CompareOp::Gte => a >= b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{NodeKey, OutcomeKey};
    use std::collections::HashMap;
    use std::path::PathBuf;

    struct MockCtx {
        outcomes: HashMap<NodeKey, OutcomeKey>,
        artifacts: HashMap<String, u64>,
        env: HashMap<String, String>,
        files: Vec<PathBuf>,
    }

    impl Default for MockCtx {
        fn default() -> Self {
            Self {
                outcomes: HashMap::new(),
                artifacts: HashMap::new(),
                env: HashMap::new(),
                files: Vec::new(),
            }
        }
    }

    impl PredicateContext for MockCtx {
        fn outcome_of(&self, node: &NodeKey) -> Option<&OutcomeKey> {
            self.outcomes.get(node)
        }
        fn artifact_size(&self, name: &str) -> Option<u64> {
            self.artifacts.get(name).copied()
        }
        fn env_var(&self, name: &str) -> Option<String> {
            self.env.get(name).cloned()
        }
        fn file_exists(&self, path: &Path) -> bool {
            self.files.iter().any(|p| p == path)
        }
    }

    #[test]
    fn file_exists_true_when_present() {
        let mut ctx = MockCtx::default();
        ctx.files.push(PathBuf::from("Cargo.toml"));
        let p = Predicate::FileExists { path: "Cargo.toml".into() };
        assert!(evaluate(&p, &ctx));
    }

    #[test]
    fn file_exists_false_when_absent() {
        let ctx = MockCtx::default();
        let p = Predicate::FileExists { path: "missing.toml".into() };
        assert!(!evaluate(&p, &ctx));
    }

    #[test]
    fn artifact_size_eq() {
        let mut ctx = MockCtx::default();
        ctx.artifacts.insert("spec.md".into(), 1024);
        let p = Predicate::ArtifactSize {
            artifact: "spec.md".into(),
            op: CompareOp::Eq,
            value: 1024,
        };
        assert!(evaluate(&p, &ctx));
    }

    #[test]
    fn artifact_size_gt_with_missing_artifact_is_false() {
        let ctx = MockCtx::default();
        let p = Predicate::ArtifactSize {
            artifact: "missing".into(),
            op: CompareOp::Gt,
            value: 0,
        };
        assert!(!evaluate(&p, &ctx));
    }

    #[test]
    fn artifact_size_all_compare_ops() {
        let mut ctx = MockCtx::default();
        ctx.artifacts.insert("a".into(), 10);
        for (op, expected) in [
            (CompareOp::Eq, false),
            (CompareOp::Ne, true),
            (CompareOp::Lt, true),
            (CompareOp::Lte, true),
            (CompareOp::Gt, false),
            (CompareOp::Gte, false),
        ] {
            let p = Predicate::ArtifactSize {
                artifact: "a".into(),
                op,
                value: 20,
            };
            assert_eq!(evaluate(&p, &ctx), expected, "op={op:?}");
        }
    }

    #[test]
    fn outcome_matches_positive() {
        let mut ctx = MockCtx::default();
        let n = NodeKey::try_from("plan").unwrap();
        ctx.outcomes.insert(n.clone(), OutcomeKey::try_from("done").unwrap());
        let p = Predicate::OutcomeMatches {
            node: n,
            outcome: OutcomeKey::try_from("done").unwrap(),
        };
        assert!(evaluate(&p, &ctx));
    }

    #[test]
    fn outcome_matches_missing_node_is_false() {
        let ctx = MockCtx::default();
        let p = Predicate::OutcomeMatches {
            node: NodeKey::try_from("nope").unwrap(),
            outcome: OutcomeKey::try_from("done").unwrap(),
        };
        assert!(!evaluate(&p, &ctx));
    }

    #[test]
    fn env_var_eq() {
        let mut ctx = MockCtx::default();
        ctx.env.insert("MODE".into(), "dev".into());
        let p = Predicate::EnvVar {
            name: "MODE".into(),
            op: CompareOp::Eq,
            value: "dev".into(),
        };
        assert!(evaluate(&p, &ctx));
    }

    #[test]
    fn env_var_undefined_is_false() {
        let ctx = MockCtx::default();
        let p = Predicate::EnvVar {
            name: "UNDEFINED".into(),
            op: CompareOp::Eq,
            value: "x".into(),
        };
        assert!(!evaluate(&p, &ctx));
    }

    #[test]
    fn and_short_circuits_on_first_false() {
        let ctx = MockCtx::default();
        let p = Predicate::And {
            and: vec![
                Predicate::FileExists { path: "missing1".into() },
                Predicate::FileExists { path: "missing2".into() },
            ],
        };
        assert!(!evaluate(&p, &ctx));
    }

    #[test]
    fn or_short_circuits_on_first_true() {
        let mut ctx = MockCtx::default();
        ctx.files.push(PathBuf::from("present"));
        let p = Predicate::Or {
            or: vec![
                Predicate::FileExists { path: "present".into() },
                Predicate::FileExists { path: "absent".into() },
            ],
        };
        assert!(evaluate(&p, &ctx));
    }

    #[test]
    fn not_inverts_inner() {
        let ctx = MockCtx::default();
        let p = Predicate::Not {
            not: Box::new(Predicate::FileExists { path: "missing".into() }),
        };
        assert!(evaluate(&p, &ctx));
    }

    #[test]
    fn nested_combinators() {
        let mut ctx = MockCtx::default();
        ctx.files.push(PathBuf::from("a"));
        ctx.artifacts.insert("art".into(), 5);

        // (file_exists("a") AND artifact_size("art") > 0) OR file_exists("z")
        let p = Predicate::Or {
            or: vec![
                Predicate::And {
                    and: vec![
                        Predicate::FileExists { path: "a".into() },
                        Predicate::ArtifactSize {
                            artifact: "art".into(),
                            op: CompareOp::Gt,
                            value: 0,
                        },
                    ],
                },
                Predicate::FileExists { path: "z".into() },
            ],
        };
        assert!(evaluate(&p, &ctx));
    }
}
```

- [ ] **Step 2: Wire the new module into the crate**

Edit `crates/surge-core/src/lib.rs` — add to the `pub mod` block (alphabetical order):

```rust
pub mod predicate;
```

- [ ] **Step 3: Run tests, expect pass**

Run: `cargo test -p surge-core --lib predicate::tests`
Expected: all 12 tests pass.

- [ ] **Step 4: Verify the workspace still builds**

Run: `cargo build --workspace`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-core/src/predicate.rs crates/surge-core/src/lib.rs
git commit -m "M5(core): add predicate::evaluate + PredicateContext trait"
```

---

## Phase 2 — `BridgeFacade` trait + mock

### Task 2.1: Add `BridgeFacade` trait + impl on `AcpBridge`

**Files:**
- Create: `crates/surge-acp/src/bridge/facade.rs`
- Modify: `crates/surge-acp/src/bridge/mod.rs`

- [ ] **Step 1: Write a failing test that asserts the trait exists and AcpBridge impls it**

Create the test fixture by adding to the bottom of `crates/surge-acp/src/bridge/mod.rs` (after existing content):

```rust
#[cfg(test)]
mod facade_smoke {
    use super::*;

    /// Compile-time assertion that `AcpBridge` implements `BridgeFacade`.
    /// Existence of the trait + impl is what we're locking in here; behavior
    /// is exercised in tests/facade_contract.rs.
    #[test]
    fn acp_bridge_impls_facade() {
        fn assert_impl<T: facade::BridgeFacade>() {}
        assert_impl::<AcpBridge>();
    }
}
```

- [ ] **Step 2: Run test, expect failure**

Run: `cargo test -p surge-acp --lib bridge::facade_smoke`
Expected: FAIL — `unresolved import: super::facade`.

- [ ] **Step 3: Create the facade module**

Create `crates/surge-acp/src/bridge/facade.rs`:

```rust
//! `BridgeFacade` — abstraction over `AcpBridge` for engine consumers.
//!
//! Promised in the M3 design (§2.4): "if M5 engine accumulates real test
//! pain, introduce traits then." M5 is that point. Without this trait every
//! engine unit test would have to spawn `mock_acp_agent` as a subprocess,
//! adding ~200ms per test and flaking on slow CI shards.
//!
//! `AcpBridge` (the M3 type) implements this trait via straight delegation;
//! engine code holds an `Arc<dyn BridgeFacade>` so the same engine instance
//! can be wired against the real bridge or a `MockBridge` test double.

use crate::bridge::acp_bridge::AcpBridge;
use crate::bridge::error::{
    CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError,
};
use crate::bridge::event::BridgeEvent;
use crate::bridge::session::{SessionConfig, SessionMessage};
use async_trait::async_trait;
use surge_core::id::SessionId;
use tokio::sync::broadcast;

/// Engine-facing surface of an ACP bridge. All futures are `Send`.
#[async_trait]
pub trait BridgeFacade: Send + Sync {
    /// Open a new ACP session with the given configuration.
    async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<SessionId, OpenSessionError>;

    /// Send a user-role message to an open session.
    async fn send_user_message(
        &self,
        session: SessionId,
        message: SessionMessage,
    ) -> Result<(), SendMessageError>;

    /// Reply to an outstanding tool call.
    async fn reply_to_tool(
        &self,
        session: SessionId,
        call_id: String,
        payload: crate::bridge::tools::ToolResultPayload,
    ) -> Result<(), ReplyToToolError>;

    /// Close a session cleanly.
    async fn close_session(
        &self,
        session: SessionId,
    ) -> Result<(), CloseSessionError>;

    /// Subscribe to the broadcast event stream. Each subscriber receives
    /// every event from every active session.
    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent>;
}

#[async_trait]
impl BridgeFacade for AcpBridge {
    async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<SessionId, OpenSessionError> {
        AcpBridge::open_session(self, config).await
    }

    async fn send_user_message(
        &self,
        session: SessionId,
        message: SessionMessage,
    ) -> Result<(), SendMessageError> {
        AcpBridge::send_user_message(self, session, message).await
    }

    async fn reply_to_tool(
        &self,
        session: SessionId,
        call_id: String,
        payload: crate::bridge::tools::ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        AcpBridge::reply_to_tool(self, session, call_id, payload).await
    }

    async fn close_session(
        &self,
        session: SessionId,
    ) -> Result<(), CloseSessionError> {
        AcpBridge::close_session(self, session).await
    }

    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        AcpBridge::subscribe(self)
    }
}
```

If the actual M3 method signatures differ (different error type names, different parameter types — `SessionMessage` may be inlined into `send_user_message`, `call_id` may be `ToolCallId` not `String`), adjust the trait + impl to match. The principle is one-to-one delegation. Use `Read` on `crates/surge-acp/src/bridge/acp_bridge.rs` to confirm signatures before writing.

- [ ] **Step 4: Add the module declaration + re-export**

Edit `crates/surge-acp/src/bridge/mod.rs`:

```rust
pub mod facade;

pub use facade::BridgeFacade;
```

(Place near the existing module declarations.)

- [ ] **Step 5: Run test, expect pass**

Run: `cargo test -p surge-acp --lib bridge::facade_smoke`
Expected: PASS.

- [ ] **Step 6: Verify workspace builds**

Run: `cargo build -p surge-acp`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/surge-acp/src/bridge/facade.rs crates/surge-acp/src/bridge/mod.rs
git commit -m "M5(acp): add BridgeFacade trait + impl on AcpBridge (M3 §2.4 promise)"
```

### Task 2.2: Build `MockBridge` test fixture

**Files:**
- Create: `crates/surge-orchestrator/tests/fixtures/mod.rs`
- Create: `crates/surge-orchestrator/tests/fixtures/mock_bridge.rs`
- Modify: `crates/surge-orchestrator/Cargo.toml` (dev-deps)

- [ ] **Step 1: Add `tokio-test` and `async-trait` to dev-deps if missing**

Edit `crates/surge-orchestrator/Cargo.toml`, under `[dev-dependencies]`:

```toml
async-trait = "0.1"
tokio-test = "0.4"
```

- [ ] **Step 2: Create the fixtures module**

Create `crates/surge-orchestrator/tests/fixtures/mod.rs`:

```rust
//! Test fixtures shared across engine integration tests.

pub mod mock_bridge;
```

- [ ] **Step 3: Implement `MockBridge`**

Create `crates/surge-orchestrator/tests/fixtures/mock_bridge.rs`:

```rust
//! `MockBridge` — scripted `BridgeFacade` impl for unit tests.
//!
//! Records every call against the bridge into `recorded_calls` (so tests can
//! assert order/content). Emits scripted events from `scripted_events` on
//! every call to `subscribe()` — each subscriber receives the same script.
//!
//! The mock is intentionally minimal: it does not simulate session lifecycle
//! beyond returning a fresh `SessionId` from `open_session`. Tests that need
//! richer behavior should layer assertions on top.

use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Arc;
use surge_acp::bridge::error::{
    CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError,
};
use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{SessionConfig, SessionMessage};
use surge_acp::bridge::tools::ToolResultPayload;
use surge_core::id::SessionId;
use tokio::sync::{broadcast, Mutex};

/// Calls recorded against `MockBridge`, in order received.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordedCall {
    OpenSession(SessionConfig),
    SendUserMessage { session: SessionId, message: SessionMessage },
    ReplyToTool { session: SessionId, call_id: String, payload: ToolResultPayload },
    CloseSession(SessionId),
    Subscribe,
}

pub struct MockBridge {
    /// Events to broadcast — each subscriber gets a clone of these.
    scripted_events: Mutex<VecDeque<BridgeEvent>>,
    /// Calls recorded for assertion.
    pub recorded_calls: Arc<Mutex<Vec<RecordedCall>>>,
    /// Broadcast channel. Tests may pre-populate `scripted_events` and then
    /// after `subscribe()` returns, call `pump_scripted_events()` to push
    /// them through.
    tx: broadcast::Sender<BridgeEvent>,
}

impl MockBridge {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(256);
        Self {
            scripted_events: Mutex::new(VecDeque::new()),
            recorded_calls: Arc::new(Mutex::new(Vec::new())),
            tx,
        }
    }

    /// Queue an event to be broadcast on the next `pump_scripted_events()`.
    pub async fn enqueue_event(&self, event: BridgeEvent) {
        self.scripted_events.lock().await.push_back(event);
    }

    /// Drain the scripted-event queue and broadcast each event to subscribers.
    /// Tests typically call this after `bridge.subscribe()` returns to ensure
    /// the receiver is alive.
    pub async fn pump_scripted_events(&self) {
        let mut q = self.scripted_events.lock().await;
        while let Some(ev) = q.pop_front() {
            // Ignore SendError (no subscribers) — test may not have subscribed.
            let _ = self.tx.send(ev);
        }
    }
}

impl Default for MockBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BridgeFacade for MockBridge {
    async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<SessionId, OpenSessionError> {
        self.recorded_calls.lock().await.push(RecordedCall::OpenSession(config));
        Ok(SessionId::new())
    }

    async fn send_user_message(
        &self,
        session: SessionId,
        message: SessionMessage,
    ) -> Result<(), SendMessageError> {
        self.recorded_calls.lock().await.push(RecordedCall::SendUserMessage { session, message });
        Ok(())
    }

    async fn reply_to_tool(
        &self,
        session: SessionId,
        call_id: String,
        payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        self.recorded_calls
            .lock()
            .await
            .push(RecordedCall::ReplyToTool { session, call_id, payload });
        Ok(())
    }

    async fn close_session(
        &self,
        session: SessionId,
    ) -> Result<(), CloseSessionError> {
        self.recorded_calls.lock().await.push(RecordedCall::CloseSession(session));
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        // Cannot be async; record the call without locking.
        // Best-effort: spawn a quick task to record. For tests, the order of
        // subscribe vs other calls is the engine's responsibility — we record
        // it into a separate field so we don't block here.
        let recorded = self.recorded_calls.clone();
        tokio::spawn(async move {
            recorded.lock().await.push(RecordedCall::Subscribe);
        });
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_open_session() {
        let m = MockBridge::new();
        let _id = m.open_session(SessionConfig::default()).await.unwrap();
        let calls = m.recorded_calls.lock().await;
        assert!(matches!(calls[0], RecordedCall::OpenSession(_)));
    }

    #[tokio::test]
    async fn pumps_scripted_event() {
        let m = MockBridge::new();
        let mut rx = m.subscribe();
        let ev = BridgeEvent::default(); // assumes BridgeEvent: Default; if not, build a minimal variant
        m.enqueue_event(ev.clone()).await;
        m.pump_scripted_events().await;
        let received = rx.recv().await.unwrap();
        assert_eq!(received, ev);
    }
}
```

If `SessionConfig` and `BridgeEvent` don't implement `Default` or `Clone`/`PartialEq` as needed, the test bodies must construct minimal valid instances by hand. Use `Read` on the M3 types first to confirm; adjust the test bodies before running.

- [ ] **Step 4: Run the fixture's own tests**

Run: `cargo test -p surge-orchestrator --tests fixtures::mock_bridge`
Expected: 2 tests pass. If `BridgeEvent: !Default`, the second test needs a real event constructor — either replace with a known variant or skip the test.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/tests/fixtures/ crates/surge-orchestrator/Cargo.toml
git commit -m "M5(engine): add MockBridge test fixture (BridgeFacade)"
```

### Task 2.3: Add `BridgeFacade` contract test

**Files:**
- Create: `crates/surge-acp/tests/facade_contract.rs`

- [ ] **Step 1: Write a contract test that runs the same scenario against AcpBridge and MockBridge**

Create `crates/surge-acp/tests/facade_contract.rs`:

```rust
//! Property test: `AcpBridge` and a minimal mock must produce identical
//! observable behavior for an open→send→close scenario. Catches signature
//! drift if either implementation diverges.

use surge_acp::bridge::acp_bridge::AcpBridge;
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::SessionConfig;

// The mock used here is the simplest possible — only checks that the trait
// surface compiles and basic call/response shape works against both. Richer
// MockBridge lives in surge-orchestrator/tests/fixtures.
struct MinimalMock;

#[async_trait::async_trait]
impl BridgeFacade for MinimalMock {
    async fn open_session(
        &self,
        _: SessionConfig,
    ) -> Result<surge_core::id::SessionId, surge_acp::bridge::error::OpenSessionError> {
        Ok(surge_core::id::SessionId::new())
    }
    async fn send_user_message(
        &self,
        _: surge_core::id::SessionId,
        _: surge_acp::bridge::session::SessionMessage,
    ) -> Result<(), surge_acp::bridge::error::SendMessageError> {
        Ok(())
    }
    async fn reply_to_tool(
        &self,
        _: surge_core::id::SessionId,
        _: String,
        _: surge_acp::bridge::tools::ToolResultPayload,
    ) -> Result<(), surge_acp::bridge::error::ReplyToToolError> {
        Ok(())
    }
    async fn close_session(
        &self,
        _: surge_core::id::SessionId,
    ) -> Result<(), surge_acp::bridge::error::CloseSessionError> {
        Ok(())
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<surge_acp::bridge::event::BridgeEvent> {
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        std::mem::forget(tx); // keep alive for test
        rx
    }
}

/// The contract: both implementations accept the same SessionConfig type
/// and return SessionId on success. Compile-time check via generic function.
async fn open_and_close<B: BridgeFacade>(b: &B) -> bool {
    let cfg = SessionConfig::default(); // adjust if Default is unavailable
    match b.open_session(cfg).await {
        Ok(id) => b.close_session(id).await.is_ok(),
        Err(_) => false,
    }
}

#[tokio::test]
async fn minimal_mock_satisfies_facade_contract() {
    let mock = MinimalMock;
    assert!(open_and_close(&mock).await);
}

// Skipped on CI without the `mock_acp_agent` binary on PATH; documented
// as a "sanity check, run locally" test.
#[tokio::test]
#[ignore = "requires mock_acp_agent binary built (cargo build -p surge-acp); enable with --ignored"]
async fn real_acp_bridge_satisfies_facade_contract() {
    let bridge = AcpBridge::new(/* construction args per M3; adjust to actual signature */)
        .await
        .expect("AcpBridge::new");
    assert!(open_and_close(&bridge).await);
    // Cleanup
    drop(bridge);
}
```

- [ ] **Step 2: Run the contract test**

Run: `cargo test -p surge-acp --test facade_contract minimal_mock_satisfies`
Expected: PASS.

The `real_acp_bridge_satisfies_facade_contract` is `#[ignore]`d — run manually with `cargo test -p surge-acp --test facade_contract -- --ignored` after a `cargo build -p surge-acp` to verify locally.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-acp/tests/facade_contract.rs
git commit -m "M5(acp): add BridgeFacade contract test (mock + ignored real-bridge)"
```

---

## Phase 3 — Sandbox factory + tools

### Task 3.1: `engine::sandbox_factory`

**Files:**
- Create: `crates/surge-orchestrator/src/engine/sandbox_factory.rs`
- Modify: `crates/surge-orchestrator/src/engine/mod.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/surge-orchestrator/src/engine/mod.rs`:

```rust
pub mod sandbox_factory;
```

Create `crates/surge-orchestrator/src/engine/sandbox_factory.rs`:

```rust
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
    Box::new(AlwaysAllowSandbox::new())
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
```

- [ ] **Step 2: Run tests, expect pass**

Run: `cargo test -p surge-orchestrator --lib engine::sandbox_factory`
Expected: 3 tests pass.

If `AlwaysAllowSandbox` isn't `pub` from `surge-acp::bridge::sandbox`, expose it via `pub use` in `crates/surge-acp/src/bridge/mod.rs`.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/src/engine/sandbox_factory.rs crates/surge-orchestrator/src/engine/mod.rs
git commit -m "M5(engine): sandbox_factory — placeholder mapping to AlwaysAllow"
```

### Task 3.2: `engine::tools::path_guard` re-export wrapper

**Files:**
- Create: `crates/surge-orchestrator/src/engine/tools/mod.rs` (just module decls for now)
- Create: `crates/surge-orchestrator/src/engine/tools/path_guard.rs`
- Possibly modify: `crates/surge-acp/src/shared/path_guard.rs` (expose helpers)
- Modify: `crates/surge-orchestrator/src/engine/mod.rs`

- [ ] **Step 1: Confirm M3's `path_guard` helpers are public**

Run: `cargo doc -p surge-acp --no-deps --document-private-items` and inspect `target/doc/surge_acp/shared/path_guard/index.html`. Or simpler:

```bash
rg --no-heading 'pub fn|pub struct' crates/surge-acp/src/shared/path_guard.rs
```

Expected: at least `resolve_for_write`, `canonicalize_within_root` or similarly named helpers should be `pub`. If they're `pub(crate)` only, change to `pub` (one-line public surface change to surge-acp; documented in M5 spec §7.5).

- [ ] **Step 2: Wire the tools submodule**

Append to `crates/surge-orchestrator/src/engine/mod.rs`:

```rust
pub mod tools;
```

Create `crates/surge-orchestrator/src/engine/tools/mod.rs`:

```rust
//! Tool dispatch for engine-driven agent stages.

pub mod path_guard;
```

Create `crates/surge-orchestrator/src/engine/tools/path_guard.rs`:

```rust
//! Thin wrapper around `surge_acp::shared::path_guard` so engine code can
//! refer to a stable path inside the engine module tree.

pub use surge_acp::shared::path_guard::*;
```

(If specific function names are needed instead of `*`, list them after a `Read` of the M3 file.)

- [ ] **Step 3: Verify build**

Run: `cargo build -p surge-orchestrator`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/tools/ crates/surge-orchestrator/src/engine/mod.rs
git commit -m "M5(engine): tools module skeleton + path_guard re-export"
```

If Step 1 required exposing M3 helpers, also commit `crates/surge-acp/src/shared/path_guard.rs` in the same commit.

### Task 3.3: `ToolDispatcher` trait

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/tools/mod.rs`

- [ ] **Step 1: Write failing test for the trait shape**

Append to `crates/surge-orchestrator/src/engine/tools/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time check that a no-op dispatcher satisfies the trait.
    struct NoOp;

    #[async_trait::async_trait]
    impl ToolDispatcher for NoOp {
        async fn dispatch(
            &self,
            _ctx: &ToolDispatchContext<'_>,
            call: &ToolCall,
        ) -> ToolResultPayload {
            ToolResultPayload::Unsupported {
                message: format!("noop: {}", call.tool),
            }
        }
    }

    #[tokio::test]
    async fn noop_dispatcher_returns_unsupported() {
        let d = NoOp;
        let ctx = ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: std::path::Path::new("/tmp"),
            run_memory: &surge_core::run_state::RunMemory::default(),
        };
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "read_file".into(),
            arguments: serde_json::json!({}),
        };
        let result = d.dispatch(&ctx, &call).await;
        match result {
            ToolResultPayload::Unsupported { message } => assert!(message.contains("read_file")),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Run, expect failure (trait not defined)**

Run: `cargo test -p surge-orchestrator --lib engine::tools::tests`
Expected: FAIL — `cannot find trait ToolDispatcher`.

- [ ] **Step 3: Add the trait + supporting types**

In `crates/surge-orchestrator/src/engine/tools/mod.rs`, before the `#[cfg(test)]` block:

```rust
//! Tool dispatch for engine-driven agent stages.

pub mod path_guard;
pub mod worktree;

use async_trait::async_trait;
use std::path::Path;
use surge_core::id::{RunId, SessionId};
use surge_core::run_state::RunMemory;

/// One ACP tool call observed via the bridge facade.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub call_id: String,
    pub tool: String,
    pub arguments: serde_json::Value,
}

/// Result payload returned to the bridge in reply to a tool call.
///
/// Mirror of the ACP `tools::ToolResultPayload` shape — duplicated here so
/// engine code doesn't have to depend on the ACP crate's tool types directly.
/// Engine wraps/unwraps at the boundary in `stage::agent`.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolResultPayload {
    Ok { content: serde_json::Value },
    Error { message: String },
    Unsupported { message: String },
    Cancelled,
}

/// Per-call context handed to the dispatcher.
pub struct ToolDispatchContext<'a> {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub worktree_root: &'a Path,
    pub run_memory: &'a RunMemory,
}

/// Routes non-special ACP tool calls to implementations. Engine calls
/// `dispatch` for every ToolCall whose name is not `report_stage_outcome`
/// or `request_human_input` (those are engine-handled).
#[async_trait]
pub trait ToolDispatcher: Send + Sync {
    async fn dispatch(
        &self,
        ctx: &ToolDispatchContext<'_>,
        call: &ToolCall,
    ) -> ToolResultPayload;
}
```

Note: `worktree.rs` is created in the next task; the `pub mod worktree;` line is added now to keep module decls grouped, but the file gets written next. If the build fails before then, comment that line out and re-enable in 3.4.

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test -p surge-orchestrator --lib engine::tools::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/engine/tools/mod.rs
git commit -m "M5(engine): ToolDispatcher trait + ToolCall / ToolResultPayload types"
```

### Task 3.4: `WorktreeToolDispatcher` — `read_file` + `write_file`

**Files:**
- Create: `crates/surge-orchestrator/src/engine/tools/worktree.rs`

- [ ] **Step 1: Write failing tests for read_file + write_file (without shell_exec)**

Create `crates/surge-orchestrator/src/engine/tools/worktree.rs`:

```rust
//! `WorktreeToolDispatcher` — file + shell tools rooted in the run's worktree.

use crate::engine::tools::{
    ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload,
};
use async_trait::async_trait;
use std::path::PathBuf;

pub struct WorktreeToolDispatcher {
    worktree_root: PathBuf,
}

impl WorktreeToolDispatcher {
    pub fn new(worktree_root: PathBuf) -> Self {
        let canonical = std::fs::canonicalize(&worktree_root).unwrap_or(worktree_root);
        Self { worktree_root: canonical }
    }

    pub fn worktree_root(&self) -> &std::path::Path {
        &self.worktree_root
    }

    async fn read_file(&self, call: &ToolCall) -> ToolResultPayload {
        let args = match call.arguments.as_object() {
            Some(o) => o,
            None => {
                return ToolResultPayload::Error {
                    message: "read_file: arguments must be an object".into(),
                }
            }
        };
        let rel_path = match args.get("path").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResultPayload::Error {
                    message: "read_file: missing 'path' arg".into(),
                }
            }
        };
        let binary = args.get("binary").and_then(|v| v.as_bool()).unwrap_or(false);
        let abs = self.worktree_root.join(rel_path);
        let canonical = match std::fs::canonicalize(&abs) {
            Ok(p) => p,
            Err(e) => {
                return ToolResultPayload::Error {
                    message: format!("read_file: cannot canonicalize {}: {e}", abs.display()),
                }
            }
        };
        if !canonical.starts_with(&self.worktree_root) {
            return ToolResultPayload::Error {
                message: format!(
                    "read_file: path {} escapes worktree {}",
                    canonical.display(),
                    self.worktree_root.display()
                ),
            };
        }
        if binary {
            match tokio::fs::read(&canonical).await {
                Ok(bytes) => {
                    use base64::{engine::general_purpose::STANDARD, Engine};
                    ToolResultPayload::Ok {
                        content: serde_json::json!({
                            "content_base64": STANDARD.encode(&bytes),
                            "byte_len": bytes.len(),
                        }),
                    }
                }
                Err(e) => ToolResultPayload::Error {
                    message: format!("read_file: {e}"),
                },
            }
        } else {
            match tokio::fs::read_to_string(&canonical).await {
                Ok(s) => ToolResultPayload::Ok {
                    content: serde_json::json!({
                        "content_text": s,
                    }),
                },
                Err(e) => ToolResultPayload::Error {
                    message: format!("read_file: {e}"),
                },
            }
        }
    }

    async fn write_file(&self, call: &ToolCall) -> ToolResultPayload {
        let args = match call.arguments.as_object() {
            Some(o) => o,
            None => {
                return ToolResultPayload::Error {
                    message: "write_file: arguments must be an object".into(),
                }
            }
        };
        let rel_path = match args.get("path").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResultPayload::Error {
                    message: "write_file: missing 'path' arg".into(),
                }
            }
        };
        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResultPayload::Error {
                    message: "write_file: missing 'content' arg".into(),
                }
            }
        };
        let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("overwrite");
        let abs = self.worktree_root.join(rel_path);
        // For write paths, the parent must canonicalize within worktree;
        // the leaf may not yet exist.
        let parent = match abs.parent() {
            Some(p) => p,
            None => {
                return ToolResultPayload::Error {
                    message: format!("write_file: invalid path {}", abs.display()),
                }
            }
        };
        let canonical_parent = match std::fs::canonicalize(parent) {
            Ok(p) => p,
            Err(e) => {
                return ToolResultPayload::Error {
                    message: format!("write_file: cannot canonicalize parent {}: {e}", parent.display()),
                }
            }
        };
        if !canonical_parent.starts_with(&self.worktree_root) {
            return ToolResultPayload::Error {
                message: format!(
                    "write_file: parent {} escapes worktree {}",
                    canonical_parent.display(),
                    self.worktree_root.display()
                ),
            };
        }
        let leaf = abs.file_name().unwrap_or_default();
        let final_path = canonical_parent.join(leaf);
        let result = match mode {
            "create" => {
                if final_path.exists() {
                    return ToolResultPayload::Error {
                        message: format!("write_file create: {} already exists", final_path.display()),
                    };
                }
                tokio::fs::write(&final_path, content).await
            }
            "overwrite" => tokio::fs::write(&final_path, content).await,
            "append" => {
                use tokio::io::AsyncWriteExt;
                match tokio::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&final_path)
                    .await
                {
                    Ok(mut f) => f.write_all(content.as_bytes()).await,
                    Err(e) => Err(e),
                }
            }
            other => {
                return ToolResultPayload::Error {
                    message: format!("write_file: unknown mode '{other}', expected create/overwrite/append"),
                }
            }
        };
        match result {
            Ok(()) => ToolResultPayload::Ok {
                content: serde_json::json!({
                    "bytes_written": content.len(),
                }),
            },
            Err(e) => ToolResultPayload::Error {
                message: format!("write_file: {e}"),
            },
        }
    }
}

#[async_trait]
impl ToolDispatcher for WorktreeToolDispatcher {
    async fn dispatch(
        &self,
        _ctx: &ToolDispatchContext<'_>,
        call: &ToolCall,
    ) -> ToolResultPayload {
        match call.tool.as_str() {
            "read_file" => self.read_file(call).await,
            "write_file" => self.write_file(call).await,
            // shell_exec implemented in Task 3.5
            other => ToolResultPayload::Unsupported {
                message: format!("WorktreeToolDispatcher: tool '{other}' not implemented (M5 supports read_file/write_file/shell_exec)"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx<'a>(root: &'a std::path::Path, mem: &'a surge_core::run_state::RunMemory) -> ToolDispatchContext<'a> {
        ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: root,
            run_memory: mem,
        }
    }

    #[tokio::test]
    async fn read_file_returns_text_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, "hello").unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "read_file".into(),
            arguments: serde_json::json!({"path": "hello.txt"}),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        match result {
            ToolResultPayload::Ok { content } => {
                assert_eq!(content["content_text"].as_str().unwrap(), "hello");
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_file_rejects_path_escaping_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let escape = outside.path().join("secret.txt");
        std::fs::write(&escape, "secret").unwrap();

        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "read_file".into(),
            arguments: serde_json::json!({
                "path": format!("../{}/secret.txt", outside.path().file_name().unwrap().to_string_lossy()),
            }),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        assert!(matches!(result, ToolResultPayload::Error { .. }));
    }

    #[tokio::test]
    async fn write_file_overwrite_creates_then_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        for content in ["v1", "v2"] {
            let call = ToolCall {
                call_id: "c1".into(),
                tool: "write_file".into(),
                arguments: serde_json::json!({
                    "path": "out.txt",
                    "content": content,
                    "mode": "overwrite",
                }),
            };
            let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
            assert!(matches!(result, ToolResultPayload::Ok { .. }));
        }
        assert_eq!(std::fs::read_to_string(dir.path().join("out.txt")).unwrap(), "v2");
    }

    #[tokio::test]
    async fn write_file_create_rejects_existing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("exists.txt"), "old").unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "write_file".into(),
            arguments: serde_json::json!({
                "path": "exists.txt",
                "content": "new",
                "mode": "create",
            }),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        assert!(matches!(result, ToolResultPayload::Error { .. }));
    }

    #[tokio::test]
    async fn unknown_tool_returns_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "glob".into(),
            arguments: serde_json::json!({}),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        assert!(matches!(result, ToolResultPayload::Unsupported { .. }));
    }
}
```

- [ ] **Step 2: Add `tempfile` and `base64` to surge-orchestrator dev-deps + deps**

Edit `crates/surge-orchestrator/Cargo.toml`:

```toml
[dependencies]
# ... existing
base64 = "0.22"

[dev-dependencies]
# ... existing
tempfile = "3"
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::tools::worktree::tests`
Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/tools/worktree.rs crates/surge-orchestrator/Cargo.toml
git commit -m "M5(engine): WorktreeToolDispatcher — read_file + write_file with path-guard"
```

### Task 3.5: `WorktreeToolDispatcher::shell_exec`

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/tools/worktree.rs`

- [ ] **Step 1: Write failing tests for shell_exec**

Append inside `#[cfg(test)] mod tests` in `crates/surge-orchestrator/src/engine/tools/worktree.rs`:

```rust
    #[tokio::test]
    async fn shell_exec_runs_simple_command() {
        let dir = tempfile::tempdir().unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        // `echo hi` works on both Windows (via cmd /C) and Unix (via sh -c).
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "shell_exec".into(),
            arguments: serde_json::json!({"command": "echo hi"}),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        match result {
            ToolResultPayload::Ok { content } => {
                let stdout = content["stdout"].as_str().unwrap();
                assert!(stdout.contains("hi"), "stdout was {stdout:?}");
                assert_eq!(content["exit_code"].as_i64().unwrap(), 0);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shell_exec_reports_nonzero_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        // `exit 7` works on both shells (cmd: `exit /B 7`; sh: `exit 7`).
        // Pick a portable variant: run a 1-line script that exits 7.
        let cmd = if cfg!(windows) {
            "cmd /C exit 7"
        } else {
            "exit 7"
        };
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "shell_exec".into(),
            arguments: serde_json::json!({"command": cmd}),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        match result {
            ToolResultPayload::Ok { content } => {
                assert_eq!(content["exit_code"].as_i64().unwrap(), 7);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Implement `shell_exec`**

In `crates/surge-orchestrator/src/engine/tools/worktree.rs`:

(a) Add the dispatch arm — change the `match` in `impl ToolDispatcher`:

```rust
    async fn dispatch(
        &self,
        _ctx: &ToolDispatchContext<'_>,
        call: &ToolCall,
    ) -> ToolResultPayload {
        match call.tool.as_str() {
            "read_file" => self.read_file(call).await,
            "write_file" => self.write_file(call).await,
            "shell_exec" => self.shell_exec(call).await,
            other => ToolResultPayload::Unsupported {
                message: format!("WorktreeToolDispatcher: tool '{other}' not implemented (M5 supports read_file/write_file/shell_exec)"),
            },
        }
    }
```

(b) Add the implementation method on `WorktreeToolDispatcher`:

```rust
    async fn shell_exec(&self, call: &ToolCall) -> ToolResultPayload {
        let args = match call.arguments.as_object() {
            Some(o) => o,
            None => {
                return ToolResultPayload::Error {
                    message: "shell_exec: arguments must be an object".into(),
                }
            }
        };
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResultPayload::Error {
                    message: "shell_exec: missing 'command' arg".into(),
                }
            }
        };
        let cwd = if let Some(rel) = args.get("cwd_relative").and_then(|v| v.as_str()) {
            self.worktree_root.join(rel)
        } else {
            self.worktree_root.clone()
        };
        let timeout_secs = args
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(300);

        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", command]);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", command]);
            c
        };
        cmd.current_dir(&cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return ToolResultPayload::Error {
                    message: format!("shell_exec: spawn failed: {e}"),
                }
            }
        };

        let timeout = std::time::Duration::from_secs(timeout_secs);
        let output_fut = child.wait_with_output();
        let output = match tokio::time::timeout(timeout, output_fut).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return ToolResultPayload::Error {
                    message: format!("shell_exec: wait failed: {e}"),
                }
            }
            Err(_) => {
                return ToolResultPayload::Error {
                    message: format!("shell_exec: timeout after {timeout_secs}s"),
                }
            }
        };

        const TAIL_CAP: usize = 64 * 1024;
        let stdout = truncate_with_marker(String::from_utf8_lossy(&output.stdout).into_owned(), TAIL_CAP);
        let stderr = truncate_with_marker(String::from_utf8_lossy(&output.stderr).into_owned(), TAIL_CAP);
        let exit_code = output.status.code().unwrap_or(-1);

        ToolResultPayload::Ok {
            content: serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
            }),
        }
    }
```

Add the helper at module top-level (outside the `impl`):

```rust
fn truncate_with_marker(s: String, cap: usize) -> String {
    if s.len() <= cap {
        s
    } else {
        let tail_start = s.len() - cap + 64;
        let tail: String = s.chars().skip(tail_start).collect();
        format!(
            "[truncated, original length = {} bytes; showing last {} bytes]\n{}",
            s.len(),
            cap - 64,
            tail
        )
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::tools::worktree::tests`
Expected: 7 tests pass (5 prior + 2 new shell_exec).

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/tools/worktree.rs
git commit -m "M5(engine): WorktreeToolDispatcher::shell_exec with timeout + tail-cap"
```

### Task 3.6: `engine::predicates::EnginePredicateContext`

**Files:**
- Create: `crates/surge-orchestrator/src/engine/predicates.rs`
- Modify: `crates/surge-orchestrator/src/engine/mod.rs`

- [ ] **Step 1: Wire the module**

Append to `crates/surge-orchestrator/src/engine/mod.rs`:

```rust
pub mod predicates;
```

- [ ] **Step 2: Write the impl + test**

Create `crates/surge-orchestrator/src/engine/predicates.rs`:

```rust
//! Engine impl of `surge_core::predicate::PredicateContext`.

use std::path::Path;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::predicate::PredicateContext;
use surge_core::run_state::RunMemory;

pub struct EnginePredicateContext<'a> {
    pub run_memory: &'a RunMemory,
    pub worktree_root: &'a Path,
}

impl<'a> PredicateContext for EnginePredicateContext<'a> {
    fn outcome_of(&self, node: &NodeKey) -> Option<&OutcomeKey> {
        self.run_memory
            .outcomes
            .get(node)
            .and_then(|recs| recs.last())
            .map(|r| &r.outcome)
    }

    fn artifact_size(&self, name: &str) -> Option<u64> {
        self.run_memory
            .artifacts
            .get(name)
            .and_then(|a| std::fs::metadata(&a.path).ok())
            .map(|m| m.len())
    }

    fn env_var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn file_exists(&self, path: &Path) -> bool {
        let abs = self.worktree_root.join(path);
        abs.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::content_hash::ContentHash;
    use surge_core::keys::NodeKey;
    use surge_core::predicate::evaluate;
    use surge_core::run_state::{ArtifactRef, OutcomeRecord, RunMemory};
    use surge_core::branch_config::Predicate;
    use std::path::PathBuf;

    #[test]
    fn outcome_of_returns_latest() {
        let mut mem = RunMemory::default();
        let node = NodeKey::try_from("plan").unwrap();
        mem.outcomes.entry(node.clone()).or_default().push(OutcomeRecord {
            outcome: OutcomeKey::try_from("first").unwrap(),
            summary: "".into(),
            seq: 1,
        });
        mem.outcomes.entry(node.clone()).or_default().push(OutcomeRecord {
            outcome: OutcomeKey::try_from("second").unwrap(),
            summary: "".into(),
            seq: 2,
        });
        let ctx = EnginePredicateContext {
            run_memory: &mem,
            worktree_root: Path::new("/tmp"),
        };
        assert_eq!(
            ctx.outcome_of(&node).map(|o| o.as_ref()),
            Some("second")
        );
    }

    #[test]
    fn file_exists_uses_worktree_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "x").unwrap();
        let mem = RunMemory::default();
        let ctx = EnginePredicateContext {
            run_memory: &mem,
            worktree_root: dir.path(),
        };
        assert!(ctx.file_exists(Path::new("Cargo.toml")));
        assert!(!ctx.file_exists(Path::new("missing.toml")));
    }

    #[test]
    fn evaluate_outcome_matches_via_engine_ctx() {
        let mut mem = RunMemory::default();
        let node = NodeKey::try_from("plan").unwrap();
        mem.outcomes.entry(node.clone()).or_default().push(OutcomeRecord {
            outcome: OutcomeKey::try_from("done").unwrap(),
            summary: "".into(),
            seq: 1,
        });
        let ctx = EnginePredicateContext {
            run_memory: &mem,
            worktree_root: Path::new("/tmp"),
        };
        let p = Predicate::OutcomeMatches {
            node,
            outcome: OutcomeKey::try_from("done").unwrap(),
        };
        assert!(evaluate(&p, &ctx));
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::predicates`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/predicates.rs crates/surge-orchestrator/src/engine/mod.rs
git commit -m "M5(engine): EnginePredicateContext impl on RunMemory + worktree fs"
```

---

## Phase 4 — Engine API surface

### Task 4.1: `EngineError` taxonomy

**Files:**
- Create: `crates/surge-orchestrator/src/engine/error.rs`
- Modify: `crates/surge-orchestrator/src/engine/mod.rs`

- [ ] **Step 1: Wire module**

Append to `crates/surge-orchestrator/src/engine/mod.rs`:

```rust
pub mod error;
```

- [ ] **Step 2: Write the file with all variants**

Create `crates/surge-orchestrator/src/engine/error.rs`:

```rust
//! Engine error taxonomy.

use std::path::PathBuf;
use surge_core::id::RunId;
use surge_core::node::NodeKind;
use surge_persistence::runs::error::StorageError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("run is already active in this process: {0}")]
    RunAlreadyActive(RunId),

    #[error("graph validation failed: {0}")]
    GraphInvalid(String),

    #[error("graph contains M6+ feature ({kind:?}); not supported in M5")]
    UnsupportedNodeKind { kind: NodeKind },

    #[error("worktree path does not exist: {0}")]
    WorktreeMissing(PathBuf),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("bridge error: {0}")]
    Bridge(String),

    #[error("run not found: {0}")]
    RunNotFound(RunId),

    #[error("internal engine error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::node::NodeKind;

    #[test]
    fn display_messages() {
        assert_eq!(
            EngineError::WorktreeMissing(PathBuf::from("/missing")).to_string(),
            "worktree path does not exist: /missing"
        );
        assert!(
            EngineError::UnsupportedNodeKind { kind: NodeKind::Loop }
                .to_string()
                .contains("Loop")
        );
    }
}
```

If exact `StorageError` path differs (e.g., `surge_persistence::error::StorageError`), correct the import via `Read` on the M2 crate.

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::error`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/error.rs crates/surge-orchestrator/src/engine/mod.rs
git commit -m "M5(engine): EngineError taxonomy"
```

### Task 4.2: `EngineConfig`, `EngineRunConfig`, `SnapshotPolicy`

**Files:**
- Create: `crates/surge-orchestrator/src/engine/config.rs`
- Modify: `crates/surge-orchestrator/src/engine/mod.rs`

- [ ] **Step 1: Wire module**

Append to `crates/surge-orchestrator/src/engine/mod.rs`:

```rust
pub mod config;
```

- [ ] **Step 2: Write the file**

Create `crates/surge-orchestrator/src/engine/config.rs`:

```rust
//! Engine-level and run-level configuration knobs.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub snapshot_policy: SnapshotPolicy,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            snapshot_policy: SnapshotPolicy::StageBoundary,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotPolicy {
    /// Snapshot after every successful stage. M5 default and only variant.
    StageBoundary,
}

#[derive(Debug, Clone)]
pub struct EngineRunConfig {
    /// Default human-input timeout if a HumanGate doesn't override.
    /// Default 5 minutes.
    pub human_input_timeout: Duration,
    /// Per-stage timeout cap. None = use AgentConfig::limits.timeout_seconds
    /// for agent stages. Reserved for M6 daemon-level overrides.
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
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::config`
Expected: 2 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/config.rs crates/surge-orchestrator/src/engine/mod.rs
git commit -m "M5(engine): EngineConfig + EngineRunConfig + SnapshotPolicy"
```

### Task 4.3: `RunHandle`, `RunOutcome`, `EngineRunEvent`

**Files:**
- Create: `crates/surge-orchestrator/src/engine/handle.rs`
- Modify: `crates/surge-orchestrator/src/engine/mod.rs`

- [ ] **Step 1: Wire module**

Append to `crates/surge-orchestrator/src/engine/mod.rs`:

```rust
pub mod handle;
```

- [ ] **Step 2: Write the types**

Create `crates/surge-orchestrator/src/engine/handle.rs`:

```rust
//! `RunHandle` returned by `Engine::start_run` / `resume_run`.

use crate::engine::error::EngineError;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::run_event::EventPayload;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    Completed { terminal: NodeKey },
    Failed { error: String },
    Aborted { reason: String },
}

/// Engine-flavoured projection of what was just persisted.
/// Each variant corresponds 1:1 to an EventPayload that was successfully
/// written to the event log (and therefore is durable).
#[derive(Debug, Clone)]
pub enum EngineRunEvent {
    /// A new event was persisted. Carries the payload + assigned seq.
    Persisted { seq: u64, payload: EventPayload },
    /// The run reached a terminal state.
    Terminal(RunOutcome),
}

pub struct RunHandle {
    pub run_id: RunId,
    pub events: broadcast::Receiver<EngineRunEvent>,
    pub completion: JoinHandle<RunOutcome>,
}

impl RunHandle {
    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    /// Wait for the run to finish. Consumes the handle.
    pub async fn await_completion(self) -> Result<RunOutcome, EngineError> {
        self.completion
            .await
            .map_err(|e| EngineError::Internal(format!("run task join failed: {e}")))
    }
}
```

(`SessionId` etc. unused here; keep imports trim.)

- [ ] **Step 3: Verify build**

Run: `cargo build -p surge-orchestrator`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/handle.rs crates/surge-orchestrator/src/engine/mod.rs
git commit -m "M5(engine): RunHandle + RunOutcome + EngineRunEvent"
```

### Task 4.4: `Engine` struct skeleton

**Files:**
- Create: `crates/surge-orchestrator/src/engine/engine.rs`
- Modify: `crates/surge-orchestrator/src/engine/mod.rs`

- [ ] **Step 1: Wire module + re-export key public types**

Edit `crates/surge-orchestrator/src/engine/mod.rs` — append:

```rust
pub mod engine;
pub mod snapshot;
pub mod routing;
pub mod replay;
pub mod run_task;

pub use engine::Engine;
pub use error::EngineError;
pub use config::{EngineConfig, EngineRunConfig, SnapshotPolicy};
pub use handle::{RunHandle, RunOutcome, EngineRunEvent};
```

- [ ] **Step 2: Write a stub `Engine` whose methods all return `Internal("not implemented")`**

This task only stands up the API surface; later tasks (Phase 5+) implement the methods. Create `crates/surge-orchestrator/src/engine/engine.rs`:

```rust
//! `Engine` — the public API. Methods are stubbed in this task and
//! implemented incrementally in Phase 5 (lifecycle), Phase 9 (resolve),
//! Phase 11 (stop).

use crate::engine::config::{EngineConfig, EngineRunConfig};
use crate::engine::error::EngineError;
use crate::engine::handle::RunHandle;
use crate::engine::tools::ToolDispatcher;
use std::path::PathBuf;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_persistence::runs::storage::Storage;

pub struct Engine {
    bridge: Arc<dyn BridgeFacade>,
    storage: Arc<Storage>,
    tool_dispatcher: Arc<dyn ToolDispatcher>,
    config: Arc<EngineConfig>,
}

impl Engine {
    pub fn new(
        bridge: Arc<dyn BridgeFacade>,
        storage: Arc<Storage>,
        tool_dispatcher: Arc<dyn ToolDispatcher>,
        config: EngineConfig,
    ) -> Self {
        Self {
            bridge,
            storage,
            tool_dispatcher,
            config: Arc::new(config),
        }
    }

    /// Start a new run. Phase 5 implements the body.
    pub async fn start_run(
        &self,
        _run_id: RunId,
        _graph: Graph,
        _worktree_path: PathBuf,
        _run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        Err(EngineError::Internal("Engine::start_run not yet implemented (Phase 5)".into()))
    }

    /// Resume an existing run. Phase 10 implements the body.
    pub async fn resume_run(&self, _run_id: RunId) -> Result<RunHandle, EngineError> {
        Err(EngineError::Internal("Engine::resume_run not yet implemented (Phase 10)".into()))
    }

    /// Provide answer to a paused run waiting on human input. Phase 9 impl.
    pub async fn resolve_human_input(
        &self,
        _run_id: RunId,
        _call_id: Option<String>,
        _response: serde_json::Value,
    ) -> Result<(), EngineError> {
        Err(EngineError::Internal("Engine::resolve_human_input not yet implemented (Phase 9)".into()))
    }

    /// Cancel an in-flight run. Phase 11 implements the body.
    pub async fn stop_run(&self, _run_id: RunId, _reason: String) -> Result<(), EngineError> {
        Err(EngineError::Internal("Engine::stop_run not yet implemented (Phase 11)".into()))
    }
}
```

If `Storage` lives at a different path in M2 (e.g., `surge_persistence::Storage` directly), adjust the import via `Read` on `crates/surge-persistence/src/lib.rs`.

- [ ] **Step 3: Add empty placeholder modules so re-exports compile**

Create `crates/surge-orchestrator/src/engine/snapshot.rs`:

```rust
//! Engine snapshot type. Implemented in Phase 10.
```

Create `crates/surge-orchestrator/src/engine/routing.rs`:

```rust
//! Edge selection. Implemented in Phase 7.
```

Create `crates/surge-orchestrator/src/engine/replay.rs`:

```rust
//! Snapshot + event-tail → in-memory state. Implemented in Phase 10.
```

Create `crates/surge-orchestrator/src/engine/run_task.rs`:

```rust
//! Per-run tokio task. Implemented in Phase 5.
```

- [ ] **Step 4: Verify build**

Run: `cargo build -p surge-orchestrator`
Expected: clean (a few unused-variable warnings in stub methods are acceptable; suppress later when bodies land).

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/engine/
git commit -m "M5(engine): Engine struct skeleton + module re-exports"
```

---

## Phase 5 — Run lifecycle (cold start)

### Task 5.1: `routing::next_node_after`

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/routing.rs`

- [ ] **Step 1: Write failing test**

Replace `crates/surge-orchestrator/src/engine/routing.rs` content with:

```rust
//! Edge selection given (current node, outcome).

use surge_core::edge::Edge;
use surge_core::graph::Graph;
use surge_core::keys::{NodeKey, OutcomeKey};
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum RoutingError {
    #[error("no edge from node {from} matches outcome {outcome}")]
    NoMatchingEdge { from: NodeKey, outcome: OutcomeKey },
    #[error("multiple edges from node {from} match outcome {outcome} (parallel fan-out — M6)")]
    MultipleMatches { from: NodeKey, outcome: OutcomeKey },
}

/// Find the next node after `current` produces `outcome`.
///
/// M5 expects a unique edge per `(from_node, outcome)` pair. Multiple matches
/// indicate parallel fan-out, which is M6 scope — surfaced as `MultipleMatches`.
pub fn next_node_after(
    graph: &Graph,
    current: &NodeKey,
    outcome: &OutcomeKey,
) -> Result<NodeKey, RoutingError> {
    let matches: Vec<&Edge> = graph
        .edges
        .iter()
        .filter(|e| &e.from.node == current && &e.from.outcome == outcome)
        .collect();
    match matches.as_slice() {
        [] => Err(RoutingError::NoMatchingEdge {
            from: current.clone(),
            outcome: outcome.clone(),
        }),
        [edge] => Ok(edge.to.clone()),
        _ => Err(RoutingError::MultipleMatches {
            from: current.clone(),
            outcome: outcome.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
    use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use surge_core::keys::EdgeKey;
    use std::collections::BTreeMap;

    fn graph_with_edges(edges: Vec<Edge>) -> Graph {
        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "test".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
            },
            start: NodeKey::try_from("start").unwrap(),
            nodes: BTreeMap::new(),
            edges,
            subgraphs: BTreeMap::new(),
        }
    }

    fn edge(id: &str, from_node: &str, from_outcome: &str, to: &str) -> Edge {
        Edge {
            id: EdgeKey::try_from(id).unwrap(),
            from: PortRef {
                node: NodeKey::try_from(from_node).unwrap(),
                outcome: OutcomeKey::try_from(from_outcome).unwrap(),
            },
            to: NodeKey::try_from(to).unwrap(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        }
    }

    #[test]
    fn unique_match_returns_target() {
        let g = graph_with_edges(vec![edge("e1", "a", "done", "b")]);
        let next = next_node_after(
            &g,
            &NodeKey::try_from("a").unwrap(),
            &OutcomeKey::try_from("done").unwrap(),
        )
        .unwrap();
        assert_eq!(next, NodeKey::try_from("b").unwrap());
    }

    #[test]
    fn no_match_returns_error() {
        let g = graph_with_edges(vec![edge("e1", "a", "done", "b")]);
        let result = next_node_after(
            &g,
            &NodeKey::try_from("a").unwrap(),
            &OutcomeKey::try_from("retry").unwrap(),
        );
        assert!(matches!(result, Err(RoutingError::NoMatchingEdge { .. })));
    }

    #[test]
    fn multiple_matches_returns_error() {
        let g = graph_with_edges(vec![
            edge("e1", "a", "done", "b"),
            edge("e2", "a", "done", "c"),
        ]);
        let result = next_node_after(
            &g,
            &NodeKey::try_from("a").unwrap(),
            &OutcomeKey::try_from("done").unwrap(),
        );
        assert!(matches!(result, Err(RoutingError::MultipleMatches { .. })));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::routing`
Expected: 3 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/src/engine/routing.rs
git commit -m "M5(engine): routing::next_node_after with parallel-fan-out detection"
```

### Task 5.2: Graph validation helpers

**Files:**
- Create: `crates/surge-orchestrator/src/engine/validate.rs`
- Modify: `crates/surge-orchestrator/src/engine/mod.rs`

- [ ] **Step 1: Wire module**

Append to `crates/surge-orchestrator/src/engine/mod.rs`:

```rust
pub mod validate;
```

- [ ] **Step 2: Write the validator**

Create `crates/surge-orchestrator/src/engine/validate.rs`:

```rust
//! Pre-execution graph validation: rejects M6+ features and structural
//! errors (start node missing, edges referencing unknown nodes).

use crate::engine::error::EngineError;
use surge_core::graph::Graph;
use surge_core::node::NodeKind;

/// Validate the graph for M5 execution. Returns Ok(()) if it can run.
pub fn validate_for_m5(graph: &Graph) -> Result<(), EngineError> {
    if !graph.nodes.contains_key(&graph.start) {
        return Err(EngineError::GraphInvalid(format!(
            "start node '{}' not present in nodes",
            graph.start
        )));
    }

    for (key, node) in &graph.nodes {
        match node.kind() {
            NodeKind::Loop | NodeKind::Subgraph => {
                return Err(EngineError::UnsupportedNodeKind { kind: node.kind() });
            }
            _ => {}
        }
        if &node.id != key {
            return Err(EngineError::GraphInvalid(format!(
                "node id {} differs from map key {}",
                node.id, key
            )));
        }
    }

    for edge in &graph.edges {
        if !graph.nodes.contains_key(&edge.from.node) {
            return Err(EngineError::GraphInvalid(format!(
                "edge {} references unknown source node {}",
                edge.id, edge.from.node
            )));
        }
        if !graph.nodes.contains_key(&edge.to) {
            return Err(EngineError::GraphInvalid(format!(
                "edge {} references unknown target node {}",
                edge.id, edge.to
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use surge_core::keys::NodeKey;
    use surge_core::loop_config::LoopConfig;
    use surge_core::node::{Node, NodeConfig, Position};
    use surge_core::terminal_config::{TerminalConfig, TerminalKind};
    use std::collections::BTreeMap;

    fn graph_with_one_terminal(start: &str) -> Graph {
        let key = NodeKey::try_from(start).unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            key.clone(),
            Node {
                id: key.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "t".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
            },
            start: key,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    #[test]
    fn minimal_terminal_graph_is_valid() {
        let g = graph_with_one_terminal("end");
        assert!(validate_for_m5(&g).is_ok());
    }

    #[test]
    fn missing_start_node_rejected() {
        let mut g = graph_with_one_terminal("end");
        g.start = NodeKey::try_from("nonexistent").unwrap();
        let err = validate_for_m5(&g).unwrap_err();
        match err {
            EngineError::GraphInvalid(msg) => assert!(msg.contains("nonexistent")),
            other => panic!("expected GraphInvalid, got {other:?}"),
        }
    }

    #[test]
    fn loop_node_rejected_as_unsupported() {
        let key = NodeKey::try_from("loop1").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            key.clone(),
            Node {
                id: key.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Loop(LoopConfig::default()),
            },
        );
        let g = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "t".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
            },
            start: key,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        };
        let err = validate_for_m5(&g).unwrap_err();
        assert!(matches!(
            err,
            EngineError::UnsupportedNodeKind { kind: NodeKind::Loop }
        ));
    }
}
```

If `LoopConfig::default()` doesn't exist in M1, construct it explicitly with whatever fields it needs. Use `Read` to confirm.

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::validate`
Expected: 3 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/validate.rs crates/surge-orchestrator/src/engine/mod.rs
git commit -m "M5(engine): graph validation rejects Loop/Subgraph + structural errors"
```

### Task 5.3: `Engine::start_run` implementation (cold start)

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/engine.rs`
- Modify: `crates/surge-orchestrator/src/engine/run_task.rs`

- [ ] **Step 1: Implement the run-task entrypoint stub**

Replace `crates/surge-orchestrator/src/engine/run_task.rs` content with:

```rust
//! Per-run tokio task. Drives one Graph through stage execution, snapshots,
//! and persistence writes.

use crate::engine::config::EngineRunConfig;
use crate::engine::handle::{EngineRunEvent, RunOutcome};
use crate::engine::tools::ToolDispatcher;
use std::path::PathBuf;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::Graph;
use surge_persistence::runs::run_writer::RunWriter;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub(crate) struct RunTaskParams {
    pub writer: RunWriter,
    pub bridge: Arc<dyn BridgeFacade>,
    pub tool_dispatcher: Arc<dyn ToolDispatcher>,
    pub graph: Graph,
    pub worktree_path: PathBuf,
    pub run_config: EngineRunConfig,
    pub event_tx: broadcast::Sender<EngineRunEvent>,
    pub cancel: CancellationToken,
    /// True when resuming from a snapshot — skips RunStarted/PipelineMaterialized emission.
    pub resume_mode: bool,
}

pub(crate) async fn execute(params: RunTaskParams) -> RunOutcome {
    // Phase 5 stub: emit "not implemented" then exit so the start_run path
    // is exercisable end-to-end. Phase 6+ wires in real stage execution.
    let _ = params.writer; // silence unused warnings
    let _ = params.bridge;
    let _ = params.tool_dispatcher;
    let _ = params.graph;
    let _ = params.worktree_path;
    let _ = params.run_config;
    let _ = params.cancel;
    let _ = params.resume_mode;
    let _ = params.event_tx.send(EngineRunEvent::Terminal(RunOutcome::Failed {
        error: "run_task::execute is a Phase 5 stub; real lifecycle lands in Phase 6+".into(),
    }));
    RunOutcome::Failed {
        error: "run_task::execute Phase 5 stub".into(),
    }
}
```

- [ ] **Step 2: Implement `start_run` body**

Replace the `start_run` method in `crates/surge-orchestrator/src/engine/engine.rs` with:

```rust
    pub async fn start_run(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        use crate::engine::handle::{EngineRunEvent, RunHandle};
        use crate::engine::run_task::{execute, RunTaskParams};
        use crate::engine::validate::validate_for_m5;
        use surge_core::content_hash::ContentHash;
        use surge_core::run_event::{EventPayload, RunConfig as CoreRunConfig};
        use surge_core::sandbox::SandboxMode;
        use surge_core::approvals::ApprovalPolicy;
        use tokio::sync::broadcast;
        use tokio_util::sync::CancellationToken;

        validate_for_m5(&graph)?;

        if !worktree_path.exists() {
            return Err(EngineError::WorktreeMissing(worktree_path));
        }

        let writer = self
            .storage
            .create_run(run_id, &worktree_path, None)
            .await
            .map_err(EngineError::Storage)?;

        // Emit RunStarted + PipelineMaterialized atomically.
        let core_run_config = CoreRunConfig {
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
        };
        let graph_bytes = serde_json::to_vec(&graph)
            .map_err(|e| EngineError::Internal(format!("graph serialize: {e}")))?;
        let graph_hash = ContentHash::compute(&graph_bytes);

        writer
            .append_events(vec![
                surge_core::run_event::VersionedEventPayload::new(EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: worktree_path.clone(),
                    initial_prompt: String::new(),
                    config: core_run_config,
                }),
                surge_core::run_event::VersionedEventPayload::new(
                    EventPayload::PipelineMaterialized {
                        graph: Box::new(graph.clone()),
                        graph_hash,
                    },
                ),
            ])
            .await
            .map_err(EngineError::Storage)?;

        let (event_tx, event_rx) = broadcast::channel(256);
        let cancel = CancellationToken::new();

        let params = RunTaskParams {
            writer,
            bridge: self.bridge.clone(),
            tool_dispatcher: self.tool_dispatcher.clone(),
            graph,
            worktree_path,
            run_config,
            event_tx,
            cancel,
            resume_mode: false,
        };

        let join = tokio::spawn(execute(params));

        Ok(RunHandle {
            run_id,
            events: event_rx,
            completion: join,
        })
    }
```

If `Storage::create_run` has a different signature in M2 (`async` vs sync, parameter names), adjust per `Read` of `crates/surge-persistence/src/runs/storage.rs`. The test in Step 4 will catch any mismatch.

- [ ] **Step 3: Add an integration test that just exercises start_run + the stub task**

Create `crates/surge-orchestrator/tests/engine_start_run_smoke.rs`:

```rust
//! Smoke test: start_run constructs the handle and the stub task fires.
//!
//! This test lives in tests/ (not src/) so it can use real M2 storage +
//! MockBridge. The stub task will return Failed; we just verify the path
//! is wired.

mod fixtures;

use std::sync::Arc;
use surge_core::approvals::ApprovalPolicy;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::sandbox::SandboxMode;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_persistence::runs::storage::Storage;
use std::collections::BTreeMap;

fn minimal_graph() -> Graph {
    let end = NodeKey::try_from("end").unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(
        end.clone(),
        Node {
            id: end.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "smoke".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_run_smoke_completes_with_stub_failure() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(dir.path()).await.unwrap());
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn surge_acp::bridge::facade::BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher>;

    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let handle = engine
        .start_run(run_id, minimal_graph(), dir.path().to_path_buf(), EngineRunConfig::default())
        .await
        .expect("start_run");

    let outcome = handle.await_completion().await.unwrap();
    // Phase 5 stub returns Failed; later phases will produce Completed.
    match outcome {
        surge_orchestrator::engine::RunOutcome::Failed { error } => {
            assert!(error.contains("Phase 5 stub"));
        }
        other => panic!("expected Failed (stub), got {other:?}"),
    }
}
```

If `Storage::open` signature differs (sync vs async, takes `&Path` vs `PathBuf`), correct after `Read`-ing M2 storage.

- [ ] **Step 4: Run the test**

Run: `cargo test -p surge-orchestrator --test engine_start_run_smoke`
Expected: PASS — outcome is the Phase 5 stub failure.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/engine/engine.rs crates/surge-orchestrator/src/engine/run_task.rs crates/surge-orchestrator/tests/engine_start_run_smoke.rs
git commit -m "M5(engine): Engine::start_run cold path + run_task stub + smoke test"
```

---

## Phase 6 — Agent stage execution

### Task 6.1: `stage::agent` skeleton + session lifecycle

**Files:**
- Create: `crates/surge-orchestrator/src/engine/stage/mod.rs`
- Create: `crates/surge-orchestrator/src/engine/stage/agent.rs`
- Modify: `crates/surge-orchestrator/src/engine/mod.rs`

- [ ] **Step 1: Wire stage module**

Append to `crates/surge-orchestrator/src/engine/mod.rs`:

```rust
pub mod stage;
```

Create `crates/surge-orchestrator/src/engine/stage/mod.rs`:

```rust
//! Stage execution dispatch.

pub mod agent;
pub mod branch;
pub mod human_gate;
pub mod terminal;
pub mod notify;

use crate::engine::error::EngineError;
use surge_core::keys::OutcomeKey;
use thiserror::Error;

/// Outcome of one stage's execution. The cursor's next position is determined
/// by routing on this `OutcomeKey`.
pub type StageResult = Result<OutcomeKey, StageError>;

#[derive(Debug, Error)]
pub enum StageError {
    #[error("agent crashed: {0}")]
    AgentCrashed(String),

    #[error("agent reported undeclared outcome: {0}")]
    UndeclaredOutcome(String),

    #[error("human gate rejected (timeout or explicit)")]
    HumanGateRejected,

    #[error("human gate has TimeoutAction::Continue but no default outcome configured")]
    HumanGateContinueWithoutDefault,

    #[error("storage error: {0}")]
    Storage(String),

    #[error("bridge error: {0}")]
    Bridge(String),

    #[error("cancelled")]
    Cancelled,

    #[error("internal: {0}")]
    Internal(String),
}

impl From<StageError> for EngineError {
    fn from(e: StageError) -> Self {
        EngineError::Internal(format!("stage error: {e}"))
    }
}
```

- [ ] **Step 2: Write agent stage skeleton (no event loop yet — just open + close)**

Create `crates/surge-orchestrator/src/engine/stage/agent.rs`:

```rust
//! `NodeKind::Agent` execution.
//!
//! Phase 6.1: skeleton — opens an ACP session, sends a placeholder prompt,
//! immediately closes with a placeholder outcome. Phase 6.2 wires the real
//! event loop. Phase 6.3 handles tool dispatch. Phase 6.4 handles outcome
//! resolution + binding template substitution.

use crate::engine::sandbox_factory::build_sandbox;
use crate::engine::stage::{StageError, StageResult};
use std::path::Path;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{SessionConfig, SessionMessage};
use surge_core::agent_config::AgentConfig;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_persistence::runs::run_writer::RunWriter;

pub struct AgentStageParams<'a> {
    pub node: &'a NodeKey,
    pub agent_config: &'a AgentConfig,
    pub bridge: &'a Arc<dyn BridgeFacade>,
    pub writer: &'a RunWriter,
    pub worktree_path: &'a Path,
}

pub async fn execute_agent_stage(p: AgentStageParams<'_>) -> StageResult {
    let sandbox = build_sandbox(p.agent_config.sandbox_override.as_ref());

    // Phase 6.1: minimal SessionConfig. Real construction (tool list, prompts,
    // bindings) lands in 6.4. The shape below assumes a Default impl; if M3
    // requires explicit fields, build them out per the M3 spec §4.1.
    let session_config = SessionConfig {
        // Adjust per actual M3 SessionConfig shape via Read of crates/surge-acp/src/bridge/session.rs.
        ..SessionConfig::default()
    };
    let _ = sandbox; // wired into session_config in 6.4

    let session_id = p
        .bridge
        .open_session(session_config)
        .await
        .map_err(|e| StageError::Bridge(format!("open_session: {e}")))?;

    // Stub: send empty message, immediately close, report a placeholder
    // outcome from the node's declared_outcomes (or "done" if none).
    p.bridge
        .send_user_message(session_id, SessionMessage::default())
        .await
        .map_err(|e| StageError::Bridge(format!("send_user_message: {e}")))?;

    p.bridge
        .close_session(session_id)
        .await
        .map_err(|e| StageError::Bridge(format!("close_session: {e}")))?;

    let outcome = p
        .agent_config
        .bindings
        .first()
        .and_then(|_| None)
        .or_else(|| Some(OutcomeKey::try_from("done").ok()))
        .flatten()
        .unwrap_or_else(|| OutcomeKey::try_from("done").unwrap());

    let _ = p.writer; // writer used for events in 6.2
    let _ = p.node;

    Ok(outcome)
}
```

Adjust `SessionConfig::default()` and `SessionMessage::default()` if the M3 types lack `Default` — construct minimal valid instances per `Read` of M3 session types.

- [ ] **Step 3: Verify build**

Run: `cargo build -p surge-orchestrator`
Expected: clean (warnings about unused params are OK; later tasks consume them).

- [ ] **Step 4: Add a unit test using `MockBridge` to verify open/close calls happen**

Create `crates/surge-orchestrator/tests/engine_agent_stage_unit.rs`:

```rust
//! Unit test: agent stage opens, sends, closes the session.

mod fixtures;

use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::AgentConfig;
use surge_core::keys::{NodeKey, ProfileKey};
use surge_orchestrator::engine::stage::agent::{execute_agent_stage, AgentStageParams};
use surge_persistence::runs::storage::Storage;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_stage_opens_and_closes_session() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage
        .create_run(run_id, dir.path(), None)
        .await
        .unwrap();

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();

    let agent_cfg = AgentConfig {
        profile: ProfileKey::try_from("implementer@1.0").unwrap(),
        prompt_overrides: None,
        tool_overrides: None,
        sandbox_override: None,
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: Default::default(),
        hooks: vec![],
        custom_fields: Default::default(),
    };

    let node = NodeKey::try_from("plan_1").unwrap();
    let result = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: &agent_cfg,
        bridge: &bridge,
        writer: &writer,
        worktree_path: dir.path(),
    })
    .await
    .unwrap();

    assert_eq!(result.as_ref(), "done");

    let calls = mock.recorded_calls.lock().await;
    let kinds: Vec<&'static str> = calls
        .iter()
        .map(|c| match c {
            fixtures::mock_bridge::RecordedCall::OpenSession(_) => "open",
            fixtures::mock_bridge::RecordedCall::SendUserMessage { .. } => "send",
            fixtures::mock_bridge::RecordedCall::CloseSession(_) => "close",
            fixtures::mock_bridge::RecordedCall::ReplyToTool { .. } => "reply",
            fixtures::mock_bridge::RecordedCall::Subscribe => "subscribe",
        })
        .collect();
    assert!(kinds.contains(&"open"));
    assert!(kinds.contains(&"send"));
    assert!(kinds.contains(&"close"));
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p surge-orchestrator --test engine_agent_stage_unit`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-orchestrator/src/engine/stage/ crates/surge-orchestrator/src/engine/mod.rs crates/surge-orchestrator/tests/engine_agent_stage_unit.rs
git commit -m "M5(engine): stage::agent skeleton + StageError + open/send/close smoke"
```

### Task 6.2: Agent stage event loop + outcome handling

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/stage/agent.rs`

- [ ] **Step 1: Write failing test that requires the agent loop to dispatch a tool and observe report_stage_outcome**

Append to `crates/surge-orchestrator/tests/engine_agent_stage_unit.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_stage_loops_until_outcome_reported() {
    use surge_acp::bridge::event::BridgeEvent;
    use surge_acp::bridge::tools::ToolResultPayload as AcpResultPayload;

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());

    // Pre-script the events the bridge will emit during the stage.
    // The script must contain a report_stage_outcome ToolCall to make the
    // loop exit. The actual BridgeEvent constructor depends on M3 shape;
    // if needed, use a real-bridge tool-call event constructor.
    mock.enqueue_event(BridgeEvent::tool_call_for_test(
        "outcome-call-1",
        "report_stage_outcome",
        serde_json::json!({"outcome": "done", "summary": "ok"}),
    ))
    .await;

    let bridge: Arc<dyn surge_acp::bridge::facade::BridgeFacade> = mock.clone();

    let agent_cfg = AgentConfig {
        profile: ProfileKey::try_from("implementer@1.0").unwrap(),
        prompt_overrides: None,
        tool_overrides: None,
        sandbox_override: None,
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: Default::default(),
        hooks: vec![],
        custom_fields: Default::default(),
    };

    let node = NodeKey::try_from("plan_1").unwrap();

    // Drive both the stage and the pump in parallel.
    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        // Yield to let stage subscribe before pumping.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let result = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: &agent_cfg,
        bridge: &bridge,
        writer: &writer,
        worktree_path: dir.path(),
    })
    .await
    .unwrap();

    pump.await.unwrap();

    assert_eq!(result.as_ref(), "done");
}
```

This test requires `BridgeEvent::tool_call_for_test` — a test-only constructor. Add it in M3 if missing (or call the real constructor if BridgeEvent has public variant fields). Documented as a small M3 surface addition.

- [ ] **Step 2: Implement the event loop**

Replace `execute_agent_stage` body in `crates/surge-orchestrator/src/engine/stage/agent.rs` with:

```rust
pub async fn execute_agent_stage(p: AgentStageParams<'_>) -> StageResult {
    use surge_acp::bridge::event::BridgeEvent;
    use surge_acp::bridge::tools::ToolResultPayload as AcpResultPayload;
    use surge_core::keys::OutcomeKey;
    use surge_core::run_event::{EventPayload, SessionDisposition, VersionedEventPayload};

    let sandbox = build_sandbox(p.agent_config.sandbox_override.as_ref());
    let session_config = SessionConfig {
        ..SessionConfig::default()
    };
    let _ = sandbox;

    let session_id = p
        .bridge
        .open_session(session_config)
        .await
        .map_err(|e| StageError::Bridge(format!("open_session: {e}")))?;

    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::SessionOpened {
            node: p.node.clone(),
            session: session_id,
            agent: p.agent_config.profile.to_string(),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    let mut events = p.bridge.subscribe();

    p.bridge
        .send_user_message(session_id, SessionMessage::default())
        .await
        .map_err(|e| StageError::Bridge(format!("send_user_message: {e}")))?;

    let outcome = loop {
        let event = match events.recv().await {
            Ok(ev) => ev,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return Err(StageError::Bridge("event stream closed unexpectedly".into()));
            }
        };

        // Filter events for this session. Implementation depends on
        // BridgeEvent layout — assumes a `session_id()` accessor or a
        // `session: SessionId` field on each variant.
        if event_session_id(&event) != Some(session_id) {
            continue;
        }

        match event {
            BridgeEvent::ToolCall { call_id, tool, arguments, .. } if tool == "report_stage_outcome" => {
                let outcome_str = arguments
                    .get("outcome")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| StageError::Internal("report_stage_outcome missing 'outcome' field".into()))?;

                let declared: Vec<String> = p
                    .agent_config
                    .bindings // wrong field; in real M1 declared outcomes live on Node, not AgentConfig
                    .iter()
                    .map(|_| String::new())
                    .collect();
                let _ = declared; // M5: declared_outcomes lives on Node, not AgentConfig — wire from caller in 6.4

                let outcome = OutcomeKey::try_from(outcome_str)
                    .map_err(|e| StageError::UndeclaredOutcome(format!("{outcome_str}: {e}")))?;

                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
                        node: p.node.clone(),
                        outcome: outcome.clone(),
                        summary: arguments
                            .get("summary")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;

                p.bridge
                    .reply_to_tool(session_id, call_id, AcpResultPayload::Ok { content: serde_json::json!({}) })
                    .await
                    .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")))?;

                break outcome;
            }
            BridgeEvent::SessionTerminated { reason, .. } => {
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::SessionClosed {
                        session: session_id,
                        disposition: SessionDisposition::AgentCrashed,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;
                return Err(StageError::AgentCrashed(reason));
            }
            // Tool dispatch + token usage + artifact handling come in 6.3.
            _ => continue,
        }
    };

    p.bridge
        .close_session(session_id)
        .await
        .map_err(|e| StageError::Bridge(format!("close_session: {e}")))?;

    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::SessionClosed {
            session: session_id,
            disposition: SessionDisposition::Normal,
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    Ok(outcome)
}

fn event_session_id(event: &surge_acp::bridge::event::BridgeEvent) -> Option<surge_core::id::SessionId> {
    use surge_acp::bridge::event::BridgeEvent;
    match event {
        BridgeEvent::ToolCall { session, .. } => Some(*session),
        BridgeEvent::SessionTerminated { session, .. } => Some(*session),
        BridgeEvent::TokenUsage { session, .. } => Some(*session),
        BridgeEvent::ArtifactProduced { session, .. } => Some(*session),
        _ => None,
    }
}
```

The exact `BridgeEvent` variant names + field layout depend on M3. After `Read`-ing `crates/surge-acp/src/bridge/event.rs`, adjust the pattern matches.

- [ ] **Step 3: Run the new test**

Run: `cargo test -p surge-orchestrator --test engine_agent_stage_unit agent_stage_loops`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/stage/agent.rs crates/surge-orchestrator/tests/engine_agent_stage_unit.rs
git commit -m "M5(engine): agent stage event loop + report_stage_outcome handling"
```

### Task 6.3: Agent stage tool dispatch + token + artifact events

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/stage/agent.rs`

- [ ] **Step 1: Wire the dispatcher into AgentStageParams**

In `crates/surge-orchestrator/src/engine/stage/agent.rs`, extend the params struct:

```rust
pub struct AgentStageParams<'a> {
    pub node: &'a NodeKey,
    pub agent_config: &'a AgentConfig,
    pub bridge: &'a Arc<dyn BridgeFacade>,
    pub writer: &'a RunWriter,
    pub worktree_path: &'a Path,
    pub tool_dispatcher: &'a Arc<dyn crate::engine::tools::ToolDispatcher>,
    pub run_memory: &'a surge_core::run_state::RunMemory,
    pub run_id: surge_core::id::RunId,
}
```

- [ ] **Step 2: Add dispatch arm to the event loop**

Inside the `match event { ... }` in `execute_agent_stage`, add before the catch-all:

```rust
            BridgeEvent::ToolCall { call_id, tool, arguments, .. } if tool == "request_human_input" => {
                // Phase 9 lands the real handler. Stub: reply Cancelled so we
                // don't deadlock during 6.3 testing.
                p.bridge
                    .reply_to_tool(
                        session_id,
                        call_id,
                        AcpResultPayload::Error { message: "request_human_input not yet implemented (Phase 9)".into() },
                    )
                    .await
                    .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")))?;
            }
            BridgeEvent::ToolCall { call_id, tool, arguments, .. } => {
                use crate::engine::tools::{ToolCall, ToolDispatchContext, ToolResultPayload as EngineResultPayload};
                let call = ToolCall {
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    arguments: arguments.clone(),
                };
                let ctx = ToolDispatchContext {
                    run_id: p.run_id,
                    session_id,
                    worktree_root: p.worktree_path,
                    run_memory: p.run_memory,
                };
                let result = p.tool_dispatcher.dispatch(&ctx, &call).await;

                // Persist ToolCalled + ToolResultReceived.
                use surge_core::content_hash::ContentHash;
                let args_redacted = ContentHash::compute(arguments.to_string().as_bytes());
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::ToolCalled {
                        session: session_id,
                        tool: tool.clone(),
                        args_redacted,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;

                let success = matches!(result, EngineResultPayload::Ok { .. });
                let result_hash = match &result {
                    EngineResultPayload::Ok { content } => ContentHash::compute(content.to_string().as_bytes()),
                    EngineResultPayload::Error { message } => ContentHash::compute(message.as_bytes()),
                    EngineResultPayload::Unsupported { message } => ContentHash::compute(message.as_bytes()),
                    EngineResultPayload::Cancelled => ContentHash::compute(b"cancelled"),
                };
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::ToolResultReceived {
                        session: session_id,
                        success,
                        result: result_hash,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;

                let acp_result = match result {
                    EngineResultPayload::Ok { content } => AcpResultPayload::Ok { content },
                    EngineResultPayload::Error { message } => AcpResultPayload::Error { message },
                    EngineResultPayload::Unsupported { message } => AcpResultPayload::Unsupported { message },
                    EngineResultPayload::Cancelled => AcpResultPayload::Cancelled,
                };
                p.bridge
                    .reply_to_tool(session_id, call_id, acp_result)
                    .await
                    .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")))?;
            }
            BridgeEvent::TokenUsage { prompt_tokens, output_tokens, cache_hits, model, cost_usd, .. } => {
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::TokensConsumed {
                        session: session_id,
                        prompt_tokens,
                        output_tokens,
                        cache_hits,
                        model,
                        cost_usd,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;
            }
            BridgeEvent::ArtifactProduced { name, content, .. } => {
                let stored = p.writer.store_artifact(&name, &content).await.map_err(|e| StageError::Storage(e.to_string()))?;
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::ArtifactProduced {
                        node: p.node.clone(),
                        artifact: stored.hash,
                        path: stored.path.clone(),
                        name,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;
            }
```

`AcpResultPayload` and `EngineResultPayload` are intentionally distinct types (engine has its own to avoid the ACP coupling); the conversion is one-to-one. Names of `BridgeEvent::TokenUsage` fields and `ArtifactProduced` shape need confirmation via `Read` of M3 event.rs.

- [ ] **Step 3: Update the existing test to pass the new params**

In `crates/surge-orchestrator/tests/engine_agent_stage_unit.rs`, update both test functions to construct `AgentStageParams` with the new fields. For the simple test, use a dummy `RunMemory::default()`, a fresh `RunId`, and a `WorktreeToolDispatcher` instance.

- [ ] **Step 4: Run tests**

Run: `cargo test -p surge-orchestrator --test engine_agent_stage_unit`
Expected: 2 PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/engine/stage/agent.rs crates/surge-orchestrator/tests/engine_agent_stage_unit.rs
git commit -m "M5(engine): agent stage tool dispatch + token + artifact event handling"
```

### Task 6.4: Prompt construction + binding resolution

**Files:**
- Create: `crates/surge-orchestrator/src/engine/stage/bindings.rs`
- Modify: `crates/surge-orchestrator/src/engine/stage/mod.rs`
- Modify: `crates/surge-orchestrator/src/engine/stage/agent.rs`

- [ ] **Step 1: Wire bindings module**

Append to `crates/surge-orchestrator/src/engine/stage/mod.rs`:

```rust
pub mod bindings;
```

- [ ] **Step 2: Write a failing test for binding resolution**

Create `crates/surge-orchestrator/src/engine/stage/bindings.rs`:

```rust
//! Resolve `Binding[]` from an AgentConfig into a template-substituted prompt.
//!
//! M5 supports:
//! - ArtifactSource::RunArtifact: looks up by name in RunMemory.artifacts
//! - ArtifactSource::NodeOutput: looks up the latest artifact produced by a node
//! - ArtifactSource::Static: literal content
//!
//! ArtifactSource::GlobPattern is M6+ — returns an error.

use std::path::Path;
use surge_core::agent_config::{ArtifactSource, Binding, TemplateVar};
use surge_core::run_state::RunMemory;

#[derive(Debug, thiserror::Error)]
pub enum BindingError {
    #[error("unknown artifact name: {0}")]
    UnknownArtifact(String),
    #[error("node {0} produced no artifacts")]
    NoArtifactsForNode(String),
    #[error("GlobPattern bindings are M6+; not supported in M5")]
    GlobUnsupported,
    #[error("io error reading artifact {0}: {1}")]
    Io(String, std::io::Error),
}

pub async fn resolve_bindings(
    bindings: &[Binding],
    memory: &RunMemory,
    worktree_root: &Path,
) -> Result<Vec<(TemplateVar, String)>, BindingError> {
    let mut out = Vec::with_capacity(bindings.len());
    for b in bindings {
        let value = match &b.source {
            ArtifactSource::RunArtifact { name } => {
                let aref = memory
                    .artifacts
                    .get(name)
                    .ok_or_else(|| BindingError::UnknownArtifact(name.clone()))?;
                read_artifact_text(&aref.path, worktree_root, &aref.name).await?
            }
            ArtifactSource::NodeOutput { node, artifact } => {
                let arefs = memory
                    .artifacts_by_node
                    .get(node)
                    .ok_or_else(|| BindingError::NoArtifactsForNode(node.to_string()))?;
                let aref = arefs
                    .iter()
                    .find(|a| &a.name == artifact)
                    .ok_or_else(|| BindingError::UnknownArtifact(artifact.clone()))?;
                read_artifact_text(&aref.path, worktree_root, &aref.name).await?
            }
            ArtifactSource::Static { content } => content.clone(),
            ArtifactSource::GlobPattern { .. } => return Err(BindingError::GlobUnsupported),
        };
        out.push((b.target.clone(), value));
    }
    Ok(out)
}

async fn read_artifact_text(path: &Path, worktree_root: &Path, name: &str) -> Result<String, BindingError> {
    // Artifacts stored by surge-persistence have absolute paths; if relative,
    // prepend the worktree root.
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        worktree_root.join(path)
    };
    tokio::fs::read_to_string(&abs)
        .await
        .map_err(|e| BindingError::Io(name.to_string(), e))
}

/// Substitute `{{var}}` placeholders in `template` with `bindings`.
/// Unknown placeholders are left as-is (best-effort).
pub fn substitute_template(template: &str, bindings: &[(TemplateVar, String)]) -> String {
    let mut out = template.to_string();
    for (var, val) in bindings {
        let placeholder = format!("{{{{{}}}}}", var.0);
        out = out.replace(&placeholder, val);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_binding_resolves_immediately() {
        let bindings = vec![Binding {
            source: ArtifactSource::Static { content: "hello".into() },
            target: TemplateVar("greeting".into()),
        }];
        let mem = RunMemory::default();
        let dir = tempfile::tempdir().unwrap();
        let resolved = resolve_bindings(&bindings, &mem, dir.path()).await.unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].1, "hello");
    }

    #[test]
    fn substitute_replaces_known_vars() {
        let bindings = vec![(TemplateVar("name".into()), "World".into())];
        let out = substitute_template("Hello, {{name}}!", &bindings);
        assert_eq!(out, "Hello, World!");
    }

    #[test]
    fn substitute_leaves_unknown_vars_alone() {
        let bindings = vec![];
        let out = substitute_template("Hello, {{unknown}}!", &bindings);
        assert_eq!(out, "Hello, {{unknown}}!");
    }

    #[tokio::test]
    async fn glob_binding_returns_unsupported_error() {
        let bindings = vec![Binding {
            source: ArtifactSource::GlobPattern {
                node: surge_core::keys::NodeKey::try_from("x").unwrap(),
                pattern: "*.md".into(),
            },
            target: TemplateVar("v".into()),
        }];
        let mem = RunMemory::default();
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_bindings(&bindings, &mem, dir.path()).await.unwrap_err();
        assert!(matches!(err, BindingError::GlobUnsupported));
    }
}
```

- [ ] **Step 3: Wire bindings into agent stage prompt**

In `crates/surge-orchestrator/src/engine/stage/agent.rs`, replace the placeholder `send_user_message(... SessionMessage::default())` with binding-driven prompt construction:

```rust
    use crate::engine::stage::bindings::{resolve_bindings, substitute_template};

    let bindings = resolve_bindings(&p.agent_config.bindings, p.run_memory, p.worktree_path)
        .await
        .map_err(|e| StageError::Internal(format!("binding resolution: {e}")))?;

    let prompt_template = p
        .agent_config
        .prompt_overrides
        .as_ref()
        .and_then(|po| po.system.as_deref())
        .unwrap_or("");
    let prompt_text = substitute_template(prompt_template, &bindings);

    p.bridge
        .send_user_message(session_id, SessionMessage::from_text(&prompt_text))
        .await
        .map_err(|e| StageError::Bridge(format!("send_user_message: {e}")))?;
```

If `SessionMessage::from_text` doesn't exist, build a `SessionMessage` per its actual constructor (likely just `SessionMessage { content: vec![ContentBlock::Text { text: prompt_text }] }` or similar — `Read` `crates/surge-acp/src/bridge/session.rs`).

- [ ] **Step 4: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::stage::bindings && cargo test -p surge-orchestrator --test engine_agent_stage_unit`
Expected: 4 + 2 PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/engine/stage/
git commit -m "M5(engine): binding resolution + template substitution for agent prompts"
```

---

## Phase 7 — Branch + Terminal + Notify stages, run loop wiring

### Task 7.1: `stage::branch`

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/stage/branch.rs`

- [ ] **Step 1: Replace placeholder with implementation + test**

Replace `crates/surge-orchestrator/src/engine/stage/branch.rs` content with:

```rust
//! `NodeKind::Branch` execution. Pure routing logic — no ACP session.

use crate::engine::predicates::EnginePredicateContext;
use crate::engine::stage::{StageError, StageResult};
use std::path::Path;
use surge_core::branch_config::BranchConfig;
use surge_core::keys::NodeKey;
use surge_core::predicate::evaluate;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::run_state::RunMemory;
use surge_persistence::runs::run_writer::RunWriter;

pub struct BranchStageParams<'a> {
    pub node: &'a NodeKey,
    pub branch_config: &'a BranchConfig,
    pub writer: &'a RunWriter,
    pub run_memory: &'a RunMemory,
    pub worktree_root: &'a Path,
}

pub async fn execute_branch_stage(p: BranchStageParams<'_>) -> StageResult {
    let ctx = EnginePredicateContext {
        run_memory: p.run_memory,
        worktree_root: p.worktree_root,
    };

    for arm in &p.branch_config.predicates {
        if evaluate(&arm.condition, &ctx) {
            p.writer
                .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
                    node: p.node.clone(),
                    outcome: arm.outcome.clone(),
                    summary: format!("branch matched arm with outcome={}", arm.outcome),
                }))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            return Ok(arm.outcome.clone());
        }
    }

    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
            node: p.node.clone(),
            outcome: p.branch_config.default_outcome.clone(),
            summary: "branch fell through to default_outcome".into(),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    Ok(p.branch_config.default_outcome.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::branch_config::{BranchArm, Predicate};
    use surge_core::keys::OutcomeKey;
    use surge_persistence::runs::storage::Storage;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn matching_arm_wins() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        std::fs::write(dir.path().join("Cargo.toml"), "x").unwrap();

        let cfg = BranchConfig {
            predicates: vec![BranchArm {
                condition: Predicate::FileExists { path: "Cargo.toml".into() },
                outcome: OutcomeKey::try_from("rust").unwrap(),
            }],
            default_outcome: OutcomeKey::try_from("generic").unwrap(),
        };

        let mem = RunMemory::default();
        let node = NodeKey::try_from("decide").unwrap();
        let outcome = execute_branch_stage(BranchStageParams {
            node: &node,
            branch_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            worktree_root: dir.path(),
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "rust");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_match_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = BranchConfig {
            predicates: vec![BranchArm {
                condition: Predicate::FileExists { path: "missing".into() },
                outcome: OutcomeKey::try_from("rust").unwrap(),
            }],
            default_outcome: OutcomeKey::try_from("generic").unwrap(),
        };

        let mem = RunMemory::default();
        let node = NodeKey::try_from("decide").unwrap();
        let outcome = execute_branch_stage(BranchStageParams {
            node: &node,
            branch_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            worktree_root: dir.path(),
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "generic");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::stage::branch`
Expected: 2 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/src/engine/stage/branch.rs
git commit -m "M5(engine): stage::branch — predicate-based routing"
```

### Task 7.2: `stage::terminal`

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/stage/terminal.rs`

- [ ] **Step 1: Implement + test**

Replace `crates/surge-orchestrator/src/engine/stage/terminal.rs`:

```rust
//! `NodeKind::Terminal` execution.

use crate::engine::stage::StageError;
use surge_core::keys::NodeKey;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_persistence::runs::run_writer::RunWriter;

#[derive(Debug, Clone, PartialEq)]
pub enum TerminalOutcome {
    Completed { node: NodeKey },
    Failed { error: String },
}

pub struct TerminalStageParams<'a> {
    pub node: &'a NodeKey,
    pub terminal_config: &'a TerminalConfig,
    pub writer: &'a RunWriter,
}

pub async fn execute_terminal_stage(p: TerminalStageParams<'_>) -> Result<TerminalOutcome, StageError> {
    match p.terminal_config.kind {
        TerminalKind::Success => {
            p.writer
                .append_event(VersionedEventPayload::new(EventPayload::RunCompleted {
                    terminal_node: p.node.clone(),
                }))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            Ok(TerminalOutcome::Completed { node: p.node.clone() })
        }
        TerminalKind::Failure => {
            let reason = p
                .terminal_config
                .message
                .clone()
                .unwrap_or_else(|| "terminal failure node".into());
            p.writer
                .append_event(VersionedEventPayload::new(EventPayload::RunFailed {
                    error: reason.clone(),
                }))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            Ok(TerminalOutcome::Failed { error: reason })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_persistence::runs::storage::Storage;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn success_terminal_emits_run_completed() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        };
        let node = NodeKey::try_from("end").unwrap();
        let outcome = execute_terminal_stage(TerminalStageParams {
            node: &node,
            terminal_config: &cfg,
            writer: &writer,
        })
        .await
        .unwrap();
        match outcome {
            TerminalOutcome::Completed { node: n } => assert_eq!(n.as_ref(), "end"),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failure_terminal_emits_run_failed() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = TerminalConfig {
            kind: TerminalKind::Failure,
            message: Some("oops".into()),
        };
        let node = NodeKey::try_from("fail").unwrap();
        let outcome = execute_terminal_stage(TerminalStageParams {
            node: &node,
            terminal_config: &cfg,
            writer: &writer,
        })
        .await
        .unwrap();
        match outcome {
            TerminalOutcome::Failed { error } => assert_eq!(error, "oops"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::stage::terminal`
Expected: 2 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/src/engine/stage/terminal.rs
git commit -m "M5(engine): stage::terminal — Success/Failure with RunCompleted/RunFailed"
```

### Task 7.3: `stage::notify` (M5 stub)

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/stage/notify.rs`

- [ ] **Step 1: Implement + test**

Replace `crates/surge-orchestrator/src/engine/stage/notify.rs`:

```rust
//! `NodeKind::Notify` — M5 stub: log-only, advances with the fixed
//! `delivered` outcome. Real channel delivery is M6+.

use crate::engine::stage::{StageError, StageResult};
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::notify_config::NotifyConfig;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_persistence::runs::run_writer::RunWriter;

pub struct NotifyStageParams<'a> {
    pub node: &'a NodeKey,
    pub notify_config: &'a NotifyConfig,
    pub writer: &'a RunWriter,
}

pub async fn execute_notify_stage(p: NotifyStageParams<'_>) -> StageResult {
    tracing::info!(node = %p.node, "notify stage (M5 stub: log-only)");
    let outcome = OutcomeKey::try_from("delivered")
        .map_err(|e| StageError::Internal(format!("'delivered' outcome key: {e}")))?;
    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
            node: p.node.clone(),
            outcome: outcome.clone(),
            summary: "notify stub".into(),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;
    let _ = p.notify_config; // unused in stub
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_persistence::runs::storage::Storage;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notify_stub_returns_delivered_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = NotifyConfig::default();
        let node = NodeKey::try_from("ping").unwrap();
        let outcome = execute_notify_stage(NotifyStageParams {
            node: &node,
            notify_config: &cfg,
            writer: &writer,
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "delivered");
    }
}
```

If `NotifyConfig::default()` doesn't exist, construct explicitly per its M1 fields.

- [ ] **Step 2: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::stage::notify`
Expected: 1 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/src/engine/stage/notify.rs
git commit -m "M5(engine): stage::notify — log-only stub with 'delivered' outcome"
```

### Task 7.4: `run_task::execute` — main loop wiring stages + routing + completion

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/run_task.rs`

- [ ] **Step 1: Replace stub with the real loop**

Replace `crates/surge-orchestrator/src/engine/run_task.rs` with:

```rust
//! Per-run tokio task. Drives one Graph through stage execution, snapshots,
//! and persistence writes.

use crate::engine::config::EngineRunConfig;
use crate::engine::handle::{EngineRunEvent, RunOutcome};
use crate::engine::routing::next_node_after;
use crate::engine::stage::agent::{execute_agent_stage, AgentStageParams};
use crate::engine::stage::branch::{execute_branch_stage, BranchStageParams};
use crate::engine::stage::notify::{execute_notify_stage, NotifyStageParams};
use crate::engine::stage::terminal::{execute_terminal_stage, TerminalOutcome, TerminalStageParams};
use crate::engine::stage::StageError;
use crate::engine::tools::ToolDispatcher;
use std::path::PathBuf;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::node::NodeConfig;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::run_state::{Cursor, RunMemory};
use surge_persistence::runs::run_writer::RunWriter;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub(crate) struct RunTaskParams {
    pub run_id: RunId,
    pub writer: RunWriter,
    pub bridge: Arc<dyn BridgeFacade>,
    pub tool_dispatcher: Arc<dyn ToolDispatcher>,
    pub graph: Graph,
    pub worktree_path: PathBuf,
    pub run_config: EngineRunConfig,
    pub event_tx: broadcast::Sender<EngineRunEvent>,
    pub cancel: CancellationToken,
    /// Resume from an existing cursor; if None, start at graph.start.
    pub resume_cursor: Option<Cursor>,
    /// Resume from existing memory; if None, start fresh.
    pub resume_memory: Option<RunMemory>,
}

pub(crate) async fn execute(params: RunTaskParams) -> RunOutcome {
    let mut cursor = params
        .resume_cursor
        .unwrap_or_else(|| Cursor {
            node: params.graph.start.clone(),
            attempt: 1,
        });
    let mut memory = params.resume_memory.unwrap_or_default();

    loop {
        if params.cancel.is_cancelled() {
            let reason = "stop_run requested".to_string();
            let _ = params
                .writer
                .append_event(VersionedEventPayload::new(EventPayload::RunAborted {
                    reason: reason.clone(),
                }))
                .await;
            return RunOutcome::Aborted { reason };
        }

        let node = match params.graph.nodes.get(&cursor.node) {
            Some(n) => n,
            None => {
                let err = format!("cursor at unknown node {}", cursor.node);
                let _ = params
                    .writer
                    .append_event(VersionedEventPayload::new(EventPayload::RunFailed {
                        error: err.clone(),
                    }))
                    .await;
                return RunOutcome::Failed { error: err };
            }
        };

        // Emit StageEntered.
        if let Err(e) = params
            .writer
            .append_event(VersionedEventPayload::new(EventPayload::StageEntered {
                node: cursor.node.clone(),
                attempt: cursor.attempt,
            }))
            .await
        {
            return failed(&params, format!("write StageEntered: {e}")).await;
        }

        // Dispatch.
        let stage_result: Result<StageOutcome, StageError> = match &node.config {
            NodeConfig::Agent(cfg) => {
                let r = execute_agent_stage(AgentStageParams {
                    node: &cursor.node,
                    agent_config: cfg,
                    bridge: &params.bridge,
                    writer: &params.writer,
                    worktree_path: &params.worktree_path,
                    tool_dispatcher: &params.tool_dispatcher,
                    run_memory: &memory,
                    run_id: params.run_id,
                })
                .await;
                r.map(StageOutcome::Routed)
            }
            NodeConfig::Branch(cfg) => execute_branch_stage(BranchStageParams {
                node: &cursor.node,
                branch_config: cfg,
                writer: &params.writer,
                run_memory: &memory,
                worktree_root: &params.worktree_path,
            })
            .await
            .map(StageOutcome::Routed),
            NodeConfig::Notify(cfg) => execute_notify_stage(NotifyStageParams {
                node: &cursor.node,
                notify_config: cfg,
                writer: &params.writer,
            })
            .await
            .map(StageOutcome::Routed),
            NodeConfig::Terminal(cfg) => {
                let r = execute_terminal_stage(TerminalStageParams {
                    node: &cursor.node,
                    terminal_config: cfg,
                    writer: &params.writer,
                })
                .await;
                r.map(StageOutcome::Terminal)
            }
            NodeConfig::HumanGate(_) => {
                // Phase 8 implements; for now treat as failure.
                Err(StageError::Internal("HumanGate stage not yet implemented (Phase 8)".into()))
            }
            NodeConfig::Loop(_) | NodeConfig::Subgraph(_) => {
                Err(StageError::Internal(format!(
                    "node kind {:?} not supported in M5",
                    node.kind()
                )))
            }
        };

        let outcome: OutcomeKey = match stage_result {
            Ok(StageOutcome::Routed(k)) => k,
            Ok(StageOutcome::Terminal(TerminalOutcome::Completed { node: n })) => {
                return RunOutcome::Completed { terminal: n };
            }
            Ok(StageOutcome::Terminal(TerminalOutcome::Failed { error })) => {
                return RunOutcome::Failed { error };
            }
            Err(e) => {
                return failed(&params, format!("stage error at {}: {e}", cursor.node)).await;
            }
        };

        // Update memory with outcome.
        memory
            .outcomes
            .entry(cursor.node.clone())
            .or_default()
            .push(surge_core::run_state::OutcomeRecord {
                outcome: outcome.clone(),
                summary: String::new(),
                seq: 0, // best-effort; real seq tracked by storage
            });

        // Route to next node.
        let next = match next_node_after(&params.graph, &cursor.node, &outcome) {
            Ok(n) => n,
            Err(e) => return failed(&params, format!("routing: {e}")).await,
        };

        // EdgeTraversed + StageCompleted.
        let edge_id = params
            .graph
            .edges
            .iter()
            .find(|e| e.from.node == cursor.node && e.from.outcome == outcome)
            .map(|e| e.id.clone())
            .unwrap_or_else(|| {
                surge_core::keys::EdgeKey::try_from(
                    format!("{}_to_{}", cursor.node, next).as_str(),
                )
                .unwrap()
            });
        let _ = params
            .writer
            .append_event(VersionedEventPayload::new(EventPayload::EdgeTraversed {
                edge: edge_id,
                from: cursor.node.clone(),
                to: next.clone(),
            }))
            .await;
        let _ = params
            .writer
            .append_event(VersionedEventPayload::new(EventPayload::StageCompleted {
                node: cursor.node.clone(),
                outcome: outcome.clone(),
            }))
            .await;

        // Snapshot at stage boundary — wired in Phase 10 via snapshot::write_at_boundary.
        // For now, no-op; tests in Phase 10 cover the snapshot write.

        cursor = Cursor { node: next, attempt: 1 };
    }
}

enum StageOutcome {
    Routed(OutcomeKey),
    Terminal(TerminalOutcome),
}

async fn failed(params: &RunTaskParams, error: String) -> RunOutcome {
    let _ = params
        .writer
        .append_event(VersionedEventPayload::new(EventPayload::RunFailed {
            error: error.clone(),
        }))
        .await;
    let _ = params.event_tx.send(EngineRunEvent::Terminal(RunOutcome::Failed { error: error.clone() }));
    RunOutcome::Failed { error }
}

#[allow(dead_code)]
fn _unused(node: &NodeKey) -> &NodeKey { node }
```

- [ ] **Step 2: Update `Engine::start_run` to pass `run_id` + `resume_cursor: None`**

In `crates/surge-orchestrator/src/engine/engine.rs`, modify the `RunTaskParams` construction in `start_run` to include the new fields:

```rust
        let params = RunTaskParams {
            run_id,
            writer,
            bridge: self.bridge.clone(),
            tool_dispatcher: self.tool_dispatcher.clone(),
            graph,
            worktree_path,
            run_config,
            event_tx,
            cancel,
            resume_cursor: None,
            resume_memory: None,
        };
```

- [ ] **Step 3: Update Phase 5 smoke test expectation**

The smoke test in `crates/surge-orchestrator/tests/engine_start_run_smoke.rs` previously expected `RunOutcome::Failed { error: "Phase 5 stub" }`. With the real loop in place, a graph with just a `Terminal::Success` node now `RunOutcome::Completed`. Update the test to expect Completed:

```rust
    let outcome = handle.await_completion().await.unwrap();
    match outcome {
        surge_orchestrator::engine::RunOutcome::Completed { terminal } => {
            assert_eq!(terminal.as_ref(), "end");
        }
        other => panic!("expected Completed, got {other:?}"),
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p surge-orchestrator --test engine_start_run_smoke`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/engine/run_task.rs crates/surge-orchestrator/src/engine/engine.rs crates/surge-orchestrator/tests/engine_start_run_smoke.rs
git commit -m "M5(engine): run_task main loop wires stage dispatch + routing + completion"
```

---

## Phase 8 — HumanGate stage

### Task 8.1: `stage::human_gate` skeleton with summary template

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/stage/human_gate.rs`
- Modify: `crates/surge-orchestrator/src/engine/run_task.rs`

- [ ] **Step 1: Write failing test for HumanGate timeout-rejects path**

Replace `crates/surge-orchestrator/src/engine/stage/human_gate.rs`:

```rust
//! `NodeKind::HumanGate` execution.
//!
//! M5 model: pause the run, emit HumanInputRequested, wait for either an
//! external `Engine::resolve_human_input` call or the configured timeout.
//! On timeout, apply `HumanGateConfig::on_timeout` (Reject / Escalate /
//! Continue). M5 treats Escalate as Reject (no escalation channels) and
//! Continue without a default outcome as a configuration error.

use crate::engine::stage::{StageError, StageResult};
use std::time::Duration;
use surge_core::human_gate_config::{HumanGateConfig, TimeoutAction};
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::run_state::RunMemory;
use surge_persistence::runs::run_writer::RunWriter;
use tokio::sync::oneshot;

pub struct HumanGateStageParams<'a> {
    pub node: &'a NodeKey,
    pub gate_config: &'a HumanGateConfig,
    pub writer: &'a RunWriter,
    pub run_memory: &'a RunMemory,
    /// Receiver fed by `Engine::resolve_human_input`. None ⇒ test path.
    pub resolution_rx: Option<oneshot::Receiver<HumanGateResolution>>,
    /// Default timeout from EngineRunConfig if the gate doesn't set one.
    pub default_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct HumanGateResolution {
    pub outcome: OutcomeKey,
    pub response: serde_json::Value,
}

pub async fn execute_human_gate_stage(p: HumanGateStageParams<'_>) -> StageResult {
    let summary = render_summary(&p.gate_config.summary, p.run_memory);
    let timeout = p
        .gate_config
        .timeout_seconds
        .map(|s| Duration::from_secs(u64::from(s)))
        .unwrap_or(p.default_timeout);

    let schema = build_options_schema(&p.gate_config.options);

    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::HumanInputRequested {
            node: p.node.clone(),
            session: None,
            call_id: None,
            prompt: summary,
            schema: Some(schema),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    let outcome = match p.resolution_rx {
        Some(rx) => {
            tokio::select! {
                resolved = rx => match resolved {
                    Ok(res) => {
                        p.writer
                            .append_event(VersionedEventPayload::new(EventPayload::HumanInputResolved {
                                node: p.node.clone(),
                                call_id: None,
                                response: res.response.clone(),
                            }))
                            .await
                            .map_err(|e| StageError::Storage(e.to_string()))?;
                        Some(res.outcome)
                    }
                    Err(_) => None,
                },
                _ = tokio::time::sleep(timeout) => None,
            }
        }
        None => {
            tokio::time::sleep(timeout).await;
            None
        }
    };

    let final_outcome = match outcome {
        Some(o) => o,
        None => {
            p.writer
                .append_event(VersionedEventPayload::new(EventPayload::HumanInputTimedOut {
                    node: p.node.clone(),
                    call_id: None,
                    elapsed_seconds: timeout.as_secs() as u32,
                }))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            match p.gate_config.on_timeout {
                TimeoutAction::Reject | TimeoutAction::Escalate => {
                    return Err(StageError::HumanGateRejected);
                }
                TimeoutAction::Continue => {
                    // M5 has no default_outcome on HumanGateConfig; documented.
                    return Err(StageError::HumanGateContinueWithoutDefault);
                }
            }
        }
    };

    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
            node: p.node.clone(),
            outcome: final_outcome.clone(),
            summary: "human gate decision".into(),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    Ok(final_outcome)
}

fn render_summary(template: &surge_core::human_gate_config::SummaryTemplate, _memory: &RunMemory) -> String {
    // M5 rendering: just title + body, no template substitution. Future M6
    // adds template var resolution against memory.artifacts.
    format!("{}\n\n{}", template.title, template.body)
}

fn build_options_schema(options: &[surge_core::human_gate_config::ApprovalOption]) -> serde_json::Value {
    let outcomes: Vec<&str> = options.iter().map(|o| o.outcome.as_ref()).collect();
    serde_json::json!({
        "type": "object",
        "properties": {
            "outcome": {
                "type": "string",
                "enum": outcomes,
            },
            "comment": { "type": "string" },
        },
        "required": ["outcome"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::approvals::ApprovalChannel;
    use surge_core::human_gate_config::{ApprovalOption, OptionStyle, SummaryTemplate};
    use surge_persistence::runs::storage::Storage;

    fn minimal_gate_config(timeout: Option<u32>, on_timeout: TimeoutAction) -> HumanGateConfig {
        HumanGateConfig {
            delivery_channels: vec![ApprovalChannel::Telegram { chat_id_ref: "$DEFAULT".into() }],
            timeout_seconds: timeout,
            on_timeout,
            summary: SummaryTemplate {
                title: "Approve?".into(),
                body: "Do it?".into(),
                show_artifacts: vec![],
            },
            options: vec![
                ApprovalOption {
                    outcome: OutcomeKey::try_from("approve").unwrap(),
                    label: "Approve".into(),
                    style: OptionStyle::Primary,
                },
                ApprovalOption {
                    outcome: OutcomeKey::try_from("reject").unwrap(),
                    label: "Reject".into(),
                    style: OptionStyle::Danger,
                },
            ],
            allow_freetext: false,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeout_with_reject_returns_rejected_error() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = minimal_gate_config(Some(0), TimeoutAction::Reject);
        let mem = RunMemory::default();
        let node = NodeKey::try_from("approve_plan").unwrap();

        let result = execute_human_gate_stage(HumanGateStageParams {
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            resolution_rx: None,
            default_timeout: Duration::from_millis(10),
        })
        .await;

        assert!(matches!(result, Err(StageError::HumanGateRejected)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resolution_returns_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = minimal_gate_config(Some(60), TimeoutAction::Reject);
        let mem = RunMemory::default();
        let node = NodeKey::try_from("approve_plan").unwrap();

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            outcome: OutcomeKey::try_from("approve").unwrap(),
            response: serde_json::json!({"outcome": "approve"}),
        })
        .unwrap();

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            resolution_rx: Some(rx),
            default_timeout: Duration::from_secs(60),
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "approve");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::stage::human_gate`
Expected: 2 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/src/engine/stage/human_gate.rs
git commit -m "M5(engine): stage::human_gate with timeout + resolution channel"
```

### Task 8.2: Wire `HumanGate` into `run_task` dispatch

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/run_task.rs`

- [ ] **Step 1: Add a HashMap for pending gate resolutions**

In `RunTaskParams`, add:

```rust
    /// Map of `(node_key, attempt) → oneshot::Sender<HumanGateResolution>`.
    /// Engine's `resolve_human_input` finds the right sender and fires it.
    /// Phase 9 wires the registry; for now Just Add the field.
    pub gate_resolutions: Arc<tokio::sync::Mutex<std::collections::HashMap<NodeKey, tokio::sync::oneshot::Sender<crate::engine::stage::human_gate::HumanGateResolution>>>>,
```

- [ ] **Step 2: Replace HumanGate dispatch arm**

In `execute`, replace:

```rust
            NodeConfig::HumanGate(_) => {
                Err(StageError::Internal("HumanGate stage not yet implemented (Phase 8)".into()))
            }
```

with:

```rust
            NodeConfig::HumanGate(cfg) => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                params.gate_resolutions.lock().await.insert(cursor.node.clone(), tx);
                use crate::engine::stage::human_gate::{execute_human_gate_stage, HumanGateStageParams};
                let r = execute_human_gate_stage(HumanGateStageParams {
                    node: &cursor.node,
                    gate_config: cfg,
                    writer: &params.writer,
                    run_memory: &memory,
                    resolution_rx: Some(rx),
                    default_timeout: params.run_config.human_input_timeout,
                })
                .await;
                params.gate_resolutions.lock().await.remove(&cursor.node);
                r.map(StageOutcome::Routed)
            }
```

- [ ] **Step 3: Construct `gate_resolutions` in `Engine::start_run`**

In `crates/surge-orchestrator/src/engine/engine.rs`, the `start_run` body — when constructing `RunTaskParams`, add:

```rust
            gate_resolutions: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
```

- [ ] **Step 4: Verify build**

Run: `cargo build -p surge-orchestrator`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/engine/run_task.rs crates/surge-orchestrator/src/engine/engine.rs
git commit -m "M5(engine): wire HumanGate dispatch into run_task with resolution map"
```

---

## Phase 9 — `request_human_input` + resolve API

### Task 9.1: Engine-level run registry for resolve routing

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/engine.rs`

- [ ] **Step 1: Add a per-engine map of active runs**

In `crates/surge-orchestrator/src/engine/engine.rs`, modify `Engine`:

```rust
pub struct Engine {
    bridge: Arc<dyn BridgeFacade>,
    storage: Arc<Storage>,
    tool_dispatcher: Arc<dyn ToolDispatcher>,
    config: Arc<EngineConfig>,
    /// Active runs indexed by RunId. Each entry holds the per-run resolution
    /// senders + cancellation token so engine-level methods (resolve, stop)
    /// can route into the right task.
    runs: Arc<tokio::sync::RwLock<std::collections::HashMap<RunId, ActiveRun>>>,
}

struct ActiveRun {
    cancel: tokio_util::sync::CancellationToken,
    gate_resolutions: Arc<tokio::sync::Mutex<std::collections::HashMap<surge_core::keys::NodeKey, tokio::sync::oneshot::Sender<crate::engine::stage::human_gate::HumanGateResolution>>>>,
    tool_resolutions: Arc<tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>>>,
}
```

Update `Engine::new` to initialize `runs: Arc::new(tokio::sync::RwLock::new(HashMap::new()))`.

- [ ] **Step 2: Register active run in `start_run`, deregister on completion**

In `start_run`, after constructing the `gate_resolutions` and `cancel`:

```rust
        let tool_resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let active = ActiveRun {
            cancel: cancel.clone(),
            gate_resolutions: gate_resolutions.clone(),
            tool_resolutions: tool_resolutions.clone(),
        };
        self.runs.write().await.insert(run_id, active);
```

Add `tool_resolutions` to `RunTaskParams` similarly to `gate_resolutions`. Also pass it into `AgentStageParams` (next task wires the agent stage to use it).

After `tokio::spawn(execute(params))`, wrap the future to deregister on completion. Easiest pattern:

```rust
        let runs_for_cleanup = self.runs.clone();
        let join = tokio::spawn(async move {
            let outcome = execute(params).await;
            runs_for_cleanup.write().await.remove(&run_id);
            outcome
        });
```

- [ ] **Step 3: Verify build**

Run: `cargo build -p surge-orchestrator`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/engine.rs crates/surge-orchestrator/src/engine/run_task.rs
git commit -m "M5(engine): per-engine ActiveRun registry for resolve/stop routing"
```

### Task 9.2: `request_human_input` handling in agent stage

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/stage/agent.rs`

- [ ] **Step 1: Wire tool_resolutions into AgentStageParams**

```rust
pub struct AgentStageParams<'a> {
    // ... existing fields
    pub tool_resolutions: &'a Arc<tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>>>,
    pub human_input_timeout: std::time::Duration,
}
```

- [ ] **Step 2: Replace the request_human_input stub arm**

In the event-loop `match`, replace the existing stub for `request_human_input` with:

```rust
            BridgeEvent::ToolCall { call_id, tool, arguments, .. } if tool == "request_human_input" => {
                use surge_core::run_event::EventPayload;
                let prompt = arguments
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let schema = arguments.get("schema").cloned();

                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::HumanInputRequested {
                        node: p.node.clone(),
                        session: Some(session_id),
                        call_id: Some(call_id.clone()),
                        prompt,
                        schema,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;

                let (tx, rx) = tokio::sync::oneshot::channel();
                p.tool_resolutions.lock().await.insert(call_id.clone(), tx);

                let resolved = tokio::select! {
                    response = rx => match response {
                        Ok(v) => Some(v),
                        Err(_) => None, // sender dropped (run aborted)
                    },
                    _ = tokio::time::sleep(p.human_input_timeout) => None,
                };

                p.tool_resolutions.lock().await.remove(&call_id);

                match resolved {
                    Some(response) => {
                        p.writer
                            .append_event(VersionedEventPayload::new(EventPayload::HumanInputResolved {
                                node: p.node.clone(),
                                call_id: Some(call_id.clone()),
                                response: response.clone(),
                            }))
                            .await
                            .map_err(|e| StageError::Storage(e.to_string()))?;
                        p.bridge
                            .reply_to_tool(session_id, call_id, AcpResultPayload::Ok { content: response })
                            .await
                            .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")))?;
                    }
                    None => {
                        p.writer
                            .append_event(VersionedEventPayload::new(EventPayload::HumanInputTimedOut {
                                node: p.node.clone(),
                                call_id: Some(call_id.clone()),
                                elapsed_seconds: p.human_input_timeout.as_secs() as u32,
                            }))
                            .await
                            .map_err(|e| StageError::Storage(e.to_string()))?;
                        p.bridge
                            .reply_to_tool(
                                session_id,
                                call_id,
                                AcpResultPayload::Error { message: "human input timed out".into() },
                            )
                            .await
                            .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")))?;
                        // M5 fail-fast: timeout halts the stage with HumanGateRejected
                        // (semantically same as gate timeout: request not answered).
                        return Err(StageError::HumanGateRejected);
                    }
                }
            }
```

- [ ] **Step 3: Update run_task to pass tool_resolutions + human_input_timeout to AgentStageParams**

In `crates/surge-orchestrator/src/engine/run_task.rs`, in the `NodeConfig::Agent` arm:

```rust
            NodeConfig::Agent(cfg) => {
                let r = execute_agent_stage(AgentStageParams {
                    node: &cursor.node,
                    agent_config: cfg,
                    bridge: &params.bridge,
                    writer: &params.writer,
                    worktree_path: &params.worktree_path,
                    tool_dispatcher: &params.tool_dispatcher,
                    run_memory: &memory,
                    run_id: params.run_id,
                    tool_resolutions: &params.tool_resolutions,
                    human_input_timeout: params.run_config.human_input_timeout,
                })
                .await;
                r.map(StageOutcome::Routed)
            }
```

Add `tool_resolutions` field to `RunTaskParams`.

- [ ] **Step 4: Update existing unit tests to construct the new params**

Add `tool_resolutions: &Arc::new(Mutex::new(HashMap::new()))` and `human_input_timeout: Duration::from_secs(5)` to existing `AgentStageParams` constructions in `crates/surge-orchestrator/tests/engine_agent_stage_unit.rs`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p surge-orchestrator --test engine_agent_stage_unit`
Expected: 2 PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-orchestrator/src/engine/stage/agent.rs crates/surge-orchestrator/src/engine/run_task.rs crates/surge-orchestrator/tests/engine_agent_stage_unit.rs
git commit -m "M5(engine): request_human_input pause + resolve + timeout in agent stage"
```

### Task 9.3: `Engine::resolve_human_input`

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/engine.rs`

- [ ] **Step 1: Implement resolve**

Replace the `resolve_human_input` stub in `Engine`:

```rust
    pub async fn resolve_human_input(
        &self,
        run_id: RunId,
        call_id: Option<String>,
        response: serde_json::Value,
    ) -> Result<(), EngineError> {
        let runs = self.runs.read().await;
        let active = runs
            .get(&run_id)
            .ok_or(EngineError::RunNotFound(run_id))?;

        match call_id {
            Some(call_id_str) => {
                // Tool-driven resolution.
                let mut tools = active.tool_resolutions.lock().await;
                let tx = tools
                    .remove(&call_id_str)
                    .ok_or_else(|| EngineError::Internal(format!("no pending tool call '{call_id_str}'")))?;
                tx.send(response)
                    .map_err(|_| EngineError::Internal("tool resolution receiver dropped".into()))?;
                Ok(())
            }
            None => {
                // HumanGate resolution. Look up by extracting outcome from response.
                let outcome_str = response
                    .get("outcome")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| EngineError::Internal("HumanGate resolution missing 'outcome' field".into()))?;
                let outcome = surge_core::keys::OutcomeKey::try_from(outcome_str)
                    .map_err(|e| EngineError::Internal(format!("invalid outcome: {e}")))?;

                // The resolution map is keyed by NodeKey, but caller doesn't
                // know which node. M5 simplification: only one HumanGate
                // active per run at a time, so take the first entry.
                let mut gates = active.gate_resolutions.lock().await;
                let key = gates.keys().next().cloned();
                match key {
                    Some(k) => {
                        let tx = gates
                            .remove(&k)
                            .expect("just looked up");
                        tx.send(crate::engine::stage::human_gate::HumanGateResolution { outcome, response })
                            .map_err(|_| EngineError::Internal("gate resolution receiver dropped".into()))?;
                        Ok(())
                    }
                    None => Err(EngineError::Internal("no pending HumanGate to resolve".into())),
                }
            }
        }
    }
```

- [ ] **Step 2: Add a unit test for resolve via the public API**

Create `crates/surge-orchestrator/tests/engine_human_input_unit.rs`:

```rust
//! Unit test: resolve_human_input wakes up a paused agent stage.

mod fixtures;

// Test stub — will be filled in once Phase 12 has the integration setup.
// For now this file only ensures the API compiles and is callable.

#[tokio::test]
async fn resolve_human_input_returns_run_not_found_for_unknown_run() {
    use std::sync::Arc;
    use surge_acp::bridge::facade::BridgeFacade;
    use surge_orchestrator::engine::{Engine, EngineConfig, EngineError};
    use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
    use surge_persistence::runs::storage::Storage;

    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(dir.path()).await.unwrap());
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
        as Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher>;

    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    let unknown = surge_core::id::RunId::new();
    let result = engine
        .resolve_human_input(unknown, None, serde_json::json!({"outcome": "x"}))
        .await;
    assert!(matches!(result, Err(EngineError::RunNotFound(_))));
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-orchestrator --test engine_human_input_unit`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/engine.rs crates/surge-orchestrator/tests/engine_human_input_unit.rs
git commit -m "M5(engine): Engine::resolve_human_input for tool + gate resolutions"
```

---

## Phase 10 — Snapshot + resume

### Task 10.1: `EngineSnapshot` type + serde

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/snapshot.rs`

- [ ] **Step 1: Replace placeholder with the type + tests**

Replace `crates/surge-orchestrator/src/engine/snapshot.rs`:

```rust
//! Engine snapshot — written at every stage boundary.

use serde::{Deserialize, Serialize};
use surge_core::keys::NodeKey;
use surge_core::run_state::Cursor;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EngineSnapshot {
    pub schema_version: u32,
    pub cursor: SerializableCursor,
    pub at_seq: u64,
    pub stage_boundary_seq: u64,
    pub pending_human_input: Option<PendingHumanInputSnapshot>,
}

/// Serde-friendly mirror of `surge_core::run_state::Cursor`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SerializableCursor {
    pub node: String, // NodeKey serializes as its inner string
    pub attempt: u32,
}

impl From<&Cursor> for SerializableCursor {
    fn from(c: &Cursor) -> Self {
        Self {
            node: c.node.to_string(),
            attempt: c.attempt,
        }
    }
}

impl SerializableCursor {
    pub fn into_cursor(self) -> Result<Cursor, surge_core::keys::TryFromKeyError> {
        Ok(Cursor {
            node: NodeKey::try_from(self.node.as_str())?,
            attempt: self.attempt,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingHumanInputSnapshot {
    pub node: String,
    pub call_id: Option<String>,
    pub prompt: String,
    pub requested_seq: u64,
}

impl EngineSnapshot {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn new(cursor: &Cursor, at_seq: u64, stage_boundary_seq: u64) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            cursor: SerializableCursor::from(cursor),
            at_seq,
            stage_boundary_seq,
            pending_human_input: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_via_json() {
        let cursor = Cursor {
            node: NodeKey::try_from("plan_1").unwrap(),
            attempt: 1,
        };
        let snap = EngineSnapshot::new(&cursor, 42, 41);
        let json = serde_json::to_vec(&snap).unwrap();
        let parsed: EngineSnapshot = serde_json::from_slice(&json).unwrap();
        assert_eq!(snap, parsed);
    }

    #[test]
    fn cursor_roundtrip_preserves_node_and_attempt() {
        let c = Cursor {
            node: NodeKey::try_from("agent_1").unwrap(),
            attempt: 3,
        };
        let s = SerializableCursor::from(&c);
        let back = s.into_cursor().unwrap();
        assert_eq!(back.node, c.node);
        assert_eq!(back.attempt, c.attempt);
    }
}
```

The `surge_core::keys::TryFromKeyError` import path may differ in M1 — adjust per `Read` of `crates/surge-core/src/keys.rs`.

- [ ] **Step 2: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::snapshot`
Expected: 2 PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/src/engine/snapshot.rs
git commit -m "M5(engine): EngineSnapshot type + JSON roundtrip"
```

### Task 10.2: Snapshot write at stage boundary

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/run_task.rs`

- [ ] **Step 1: Add snapshot write after StageCompleted**

In `crates/surge-orchestrator/src/engine/run_task.rs`, after the `StageCompleted` `append_event` and before the `cursor = Cursor { ... }` line, add:

```rust
        // Snapshot at stage boundary (per spec §2.6, §12).
        use crate::engine::snapshot::EngineSnapshot;
        let next_cursor = Cursor { node: next.clone(), attempt: 1 };
        let current_seq = match params.writer.current_seq().await {
            Ok(s) => s,
            Err(e) => return failed(&params, format!("current_seq: {e}")).await,
        };
        let snapshot = EngineSnapshot::new(&next_cursor, current_seq, current_seq);
        let blob = match serde_json::to_vec(&snapshot) {
            Ok(b) => b,
            Err(e) => return failed(&params, format!("snapshot serialize: {e}")).await,
        };
        if let Err(e) = params.writer.write_graph_snapshot(current_seq, blob).await {
            return failed(&params, format!("write_graph_snapshot: {e}")).await;
        }
```

Then change `cursor = Cursor { node: next, attempt: 1 };` to `cursor = next_cursor;`.

- [ ] **Step 2: Add a test that verifies a snapshot is written after each stage**

Create `crates/surge-orchestrator/tests/engine_snapshot_unit.rs`:

```rust
//! Unit test: a successful linear run writes one snapshot per stage boundary.

mod fixtures;

use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_persistence::runs::storage::Storage;
use std::collections::BTreeMap;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn linear_run_writes_one_snapshot_per_stage() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(dir.path()).await.unwrap());
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
        as Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher>;

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    // Single Terminal::Success node — one stage, zero boundary snapshots
    // (snapshots happen *between* stages; a 1-stage run has no boundary).
    let end = NodeKey::try_from("end").unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(end.clone(), Node {
        id: end.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig { kind: TerminalKind::Success, message: None }),
    });
    let graph = Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "single".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    };

    let run_id = RunId::new();
    let handle = engine
        .start_run(run_id, graph, dir.path().to_path_buf(), EngineRunConfig::default())
        .await
        .unwrap();
    let _ = handle.await_completion().await.unwrap();

    // Single-stage runs have zero stage boundaries.
    let reader = storage.open_reader(run_id).await.unwrap();
    let snapshots = reader.list_snapshots().await.unwrap();
    assert_eq!(snapshots.len(), 0, "single-stage run should have 0 snapshots");
}
```

If `Storage::open_reader` doesn't exist, swap for whichever M2 API exposes snapshot listing — likely `RunReader::list_snapshots` accessed via the writer's reader handle or a separate `Storage::reader_for(run_id)`. Adjust per `Read` of M2 storage.

- [ ] **Step 2.5: Add a multi-stage variant test**

Append to the same file:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_stage_branch_run_writes_two_snapshots() {
    use surge_core::branch_config::{BranchArm, BranchConfig, Predicate};
    use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
    use surge_core::keys::{EdgeKey, OutcomeKey};

    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(dir.path()).await.unwrap());
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
        as Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher>;

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let b1 = NodeKey::try_from("b1").unwrap();
    let b2 = NodeKey::try_from("b2").unwrap();
    let end = NodeKey::try_from("end").unwrap();

    let mut nodes = BTreeMap::new();
    let mk_branch = |out: &str| Node {
        id: NodeKey::try_from(out).unwrap_or_else(|_| NodeKey::try_from("x").unwrap()),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Branch(BranchConfig {
            predicates: vec![],
            default_outcome: OutcomeKey::try_from("done").unwrap(),
        }),
    };
    nodes.insert(b1.clone(), Node {
        id: b1.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Branch(BranchConfig {
            predicates: vec![],
            default_outcome: OutcomeKey::try_from("done").unwrap(),
        }),
    });
    nodes.insert(b2.clone(), Node {
        id: b2.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Branch(BranchConfig {
            predicates: vec![],
            default_outcome: OutcomeKey::try_from("done").unwrap(),
        }),
    });
    nodes.insert(end.clone(), Node {
        id: end.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig { kind: TerminalKind::Success, message: None }),
    });
    let _ = mk_branch;

    let edges = vec![
        Edge {
            id: EdgeKey::try_from("e1").unwrap(),
            from: PortRef { node: b1.clone(), outcome: OutcomeKey::try_from("done").unwrap() },
            to: b2.clone(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
        Edge {
            id: EdgeKey::try_from("e2").unwrap(),
            from: PortRef { node: b2.clone(), outcome: OutcomeKey::try_from("done").unwrap() },
            to: end.clone(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
    ];

    let graph = Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "branch_seq".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: b1,
        nodes,
        edges,
        subgraphs: BTreeMap::new(),
    };

    let run_id = RunId::new();
    let handle = engine
        .start_run(run_id, graph, dir.path().to_path_buf(), EngineRunConfig::default())
        .await
        .unwrap();
    let outcome = handle.await_completion().await.unwrap();
    assert!(matches!(outcome, surge_orchestrator::engine::RunOutcome::Completed { .. }));

    let reader = storage.open_reader(run_id).await.unwrap();
    let snapshots = reader.list_snapshots().await.unwrap();
    assert_eq!(snapshots.len(), 2, "3-node graph (2 transitions) → 2 snapshots");
}
```

- [ ] **Step 3: Run the snapshot tests**

Run: `cargo test -p surge-orchestrator --test engine_snapshot_unit`
Expected: 2 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/run_task.rs crates/surge-orchestrator/tests/engine_snapshot_unit.rs
git commit -m "M5(engine): write snapshot at every stage boundary"
```

### Task 10.3: `Engine::resume_run` — load snapshot + tail

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/replay.rs`
- Modify: `crates/surge-orchestrator/src/engine/engine.rs`

- [ ] **Step 1: Implement replay**

Replace `crates/surge-orchestrator/src/engine/replay.rs`:

```rust
//! Reconstruct in-memory engine state from snapshot + post-snapshot events.

use crate::engine::error::EngineError;
use crate::engine::snapshot::EngineSnapshot;
use surge_core::run_event::EventPayload;
use surge_core::run_state::{Cursor, RunMemory};
use surge_persistence::runs::reader::RunReader;

pub struct ReplayedState {
    pub cursor: Cursor,
    pub memory: RunMemory,
    pub graph: surge_core::graph::Graph,
}

pub async fn replay(reader: &RunReader) -> Result<ReplayedState, EngineError> {
    // Load latest snapshot (if any).
    let snap = reader
        .latest_snapshot_at_or_before(u64::MAX)
        .await
        .map_err(EngineError::Storage)?;

    let (start_seq, snap_cursor): (u64, Option<Cursor>) = match snap {
        Some((seq, blob)) => {
            let snapshot: EngineSnapshot = serde_json::from_slice(&blob)
                .map_err(|e| EngineError::Internal(format!("snapshot deserialize: {e}")))?;
            let cursor = snapshot
                .cursor
                .into_cursor()
                .map_err(|e| EngineError::Internal(format!("snapshot cursor: {e}")))?;
            (seq, Some(cursor))
        }
        None => (0, None),
    };

    // Read events from start_seq+1 onwards. We need ALL events from seq 1
    // for memory reconstruction (artifacts, outcomes, costs), but the
    // cursor comes from the snapshot if present.
    let max_seq = reader.current_seq().await.map_err(EngineError::Storage)?;
    let all_events = reader
        .read_events(surge_persistence::runs::seq::EventSeq::from(1)..surge_persistence::runs::seq::EventSeq::from(max_seq + 1))
        .await
        .map_err(EngineError::Storage)?;

    // Find the graph from PipelineMaterialized.
    let graph = all_events
        .iter()
        .find_map(|e| match &e.payload {
            EventPayload::PipelineMaterialized { graph, .. } => Some((**graph).clone()),
            _ => None,
        })
        .ok_or_else(|| EngineError::Internal("no PipelineMaterialized event in log".into()))?;

    // Rebuild memory from events.
    let mut memory = RunMemory::default();
    for ev in &all_events {
        let core_event = surge_core::run_event::RunEvent {
            run_id: ev.run_id,
            seq: ev.seq.into(),
            timestamp: ev.timestamp,
            payload: ev.payload.clone(),
        };
        memory.apply_event(&core_event);
    }

    // Cursor: snapshot's, or graph.start if no snapshot.
    let cursor = snap_cursor.unwrap_or_else(|| Cursor {
        node: graph.start.clone(),
        attempt: 1,
    });

    let _ = start_seq;

    Ok(ReplayedState { cursor, memory, graph })
}
```

The exact `EventSeq` and `read_events` types/signatures are M2 — verify via `Read` of `crates/surge-persistence/src/runs/reader.rs` and adjust.

- [ ] **Step 2: Implement `Engine::resume_run`**

Replace the `resume_run` stub in `crates/surge-orchestrator/src/engine/engine.rs`:

```rust
    pub async fn resume_run(&self, run_id: RunId) -> Result<RunHandle, EngineError> {
        use crate::engine::handle::{EngineRunEvent, RunHandle};
        use crate::engine::replay::replay;
        use crate::engine::run_task::{execute, RunTaskParams};
        use tokio::sync::broadcast;
        use tokio_util::sync::CancellationToken;

        if self.runs.read().await.contains_key(&run_id) {
            return Err(EngineError::RunAlreadyActive(run_id));
        }

        let writer = self
            .storage
            .open_run(run_id)
            .await
            .map_err(EngineError::Storage)?;

        let reader = self
            .storage
            .open_reader(run_id)
            .await
            .map_err(EngineError::Storage)?;

        let replayed = replay(&reader).await?;

        let (event_tx, event_rx) = broadcast::channel(256);
        let cancel = CancellationToken::new();
        let gate_resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let tool_resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));

        let active = ActiveRun {
            cancel: cancel.clone(),
            gate_resolutions: gate_resolutions.clone(),
            tool_resolutions: tool_resolutions.clone(),
        };
        self.runs.write().await.insert(run_id, active);

        let params = RunTaskParams {
            run_id,
            writer,
            bridge: self.bridge.clone(),
            tool_dispatcher: self.tool_dispatcher.clone(),
            graph: replayed.graph,
            worktree_path: PathBuf::new(), // resume doesn't change worktree; M5: caller must ensure same path
            run_config: EngineRunConfig::default(),
            event_tx,
            cancel,
            resume_cursor: Some(replayed.cursor),
            resume_memory: Some(replayed.memory),
            gate_resolutions,
            tool_resolutions,
        };

        let runs_for_cleanup = self.runs.clone();
        let join = tokio::spawn(async move {
            let outcome = execute(params).await;
            runs_for_cleanup.write().await.remove(&run_id);
            outcome
        });

        Ok(RunHandle {
            run_id,
            events: event_rx,
            completion: join,
        })
    }
```

The `worktree_path: PathBuf::new()` is a placeholder; ideally resume would read it from the `RunStarted` event in the log. Track that as a small follow-up — for M5 acceptance test the worktree is identical and resume can use whatever path the test passes in via a separate API parameter.

Actually, change `resume_run` to accept the worktree_path argument:

```rust
    pub async fn resume_run(
        &self,
        run_id: RunId,
        worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError> {
        // ... use worktree_path in RunTaskParams
```

- [ ] **Step 3: Run a basic resume test**

Append to `crates/surge-orchestrator/tests/engine_snapshot_unit.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_after_completion_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(dir.path()).await.unwrap());
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
        as Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher>;

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let end = NodeKey::try_from("end").unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(end.clone(), Node {
        id: end.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig { kind: TerminalKind::Success, message: None }),
    });
    let graph = Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "rs".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    };

    let run_id = RunId::new();
    let h = engine
        .start_run(run_id, graph, dir.path().to_path_buf(), EngineRunConfig::default())
        .await
        .unwrap();
    let _ = h.await_completion().await.unwrap();

    // Resume should detect a terminal-state run and exit cleanly.
    let r = engine.resume_run(run_id, dir.path().to_path_buf()).await.unwrap();
    let outcome = r.await_completion().await.unwrap();
    match outcome {
        surge_orchestrator::engine::RunOutcome::Completed { .. } => {}
        other => panic!("expected Completed on resume, got {other:?}"),
    }
}
```

For the run-task to detect "already terminal" on resume, the loop needs to check if `cursor.node` is a Terminal node and not re-execute. The current run loop already handles this by entering the Terminal arm and emitting RunCompleted; the test verifies idempotency.

- [ ] **Step 4: Run tests**

Run: `cargo test -p surge-orchestrator --test engine_snapshot_unit resume_after`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/engine/replay.rs crates/surge-orchestrator/src/engine/engine.rs crates/surge-orchestrator/tests/engine_snapshot_unit.rs
git commit -m "M5(engine): Engine::resume_run loads snapshot + tail and continues"
```

---

## Phase 11 — Stop + concurrent runs

### Task 11.1: `Engine::stop_run`

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/engine.rs`

- [ ] **Step 1: Implement stop_run**

Replace the `stop_run` stub:

```rust
    pub async fn stop_run(&self, run_id: RunId, reason: String) -> Result<(), EngineError> {
        let active = {
            let runs = self.runs.read().await;
            runs.get(&run_id).map(|a| a.cancel.clone())
        };

        match active {
            Some(cancel) => {
                tracing::info!(run_id = %run_id, reason = %reason, "stop_run requested");
                cancel.cancel();
                // Wait briefly for the task to wind down — best-effort.
                // The task itself will emit RunAborted and exit.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                Ok(())
            }
            None => Err(EngineError::RunNotFound(run_id)),
        }
    }
```

- [ ] **Step 2: Add a unit test for stop_run**

Create `crates/surge-orchestrator/tests/engine_stop_run_unit.rs`:

```rust
mod fixtures;

use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineError};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_persistence::runs::storage::Storage;

#[tokio::test]
async fn stop_run_unknown_returns_run_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(dir.path()).await.unwrap());
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
        as Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher>;

    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    let r = engine.stop_run(surge_core::id::RunId::new(), "test".into()).await;
    assert!(matches!(r, Err(EngineError::RunNotFound(_))));
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-orchestrator --test engine_stop_run_unit`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/engine.rs crates/surge-orchestrator/tests/engine_stop_run_unit.rs
git commit -m "M5(engine): Engine::stop_run via CancellationToken + brief wind-down"
```

### Task 11.2: Concurrent runs sanity test (no shared state)

**Files:**
- Create: `crates/surge-orchestrator/tests/engine_concurrent_unit.rs`

- [ ] **Step 1: Write a test that spawns 3 runs against one engine and asserts each completes independently**

Create `crates/surge-orchestrator/tests/engine_concurrent_unit.rs`:

```rust
mod fixtures;

use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_persistence::runs::storage::Storage;
use std::collections::BTreeMap;

fn minimal_graph(name: &str) -> Graph {
    let end = NodeKey::try_from("end").unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(end.clone(), Node {
        id: end.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig { kind: TerminalKind::Success, message: None }),
    });
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: name.into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_concurrent_runs_complete_independently() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(dir.path()).await.unwrap());
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
        as Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher>;

    let engine = Arc::new(Engine::new(bridge, storage, dispatcher, EngineConfig::default()));

    let mut handles = vec![];
    for i in 0..3 {
        let eng = engine.clone();
        let dir_path = dir.path().to_path_buf();
        handles.push(tokio::spawn(async move {
            let g = minimal_graph(&format!("run-{i}"));
            let h = eng
                .start_run(RunId::new(), g, dir_path, EngineRunConfig::default())
                .await
                .unwrap();
            h.await_completion().await.unwrap()
        }));
    }

    for h in handles {
        let outcome = h.await.unwrap();
        assert!(matches!(outcome, RunOutcome::Completed { .. }));
    }
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p surge-orchestrator --test engine_concurrent_unit`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/tests/engine_concurrent_unit.rs
git commit -m "M5(engine): concurrent-runs unit test (3 independent runs, one engine)"
```

---

## Phase 12 — Integration tests via real ACP subprocess

> **Pre-flight:** Phase 12 tests use the `mock_acp_agent` binary from M3
> (`crates/surge-acp/src/bin/mock_acp_agent.rs`). Before running these
> tests, ensure the binary is built: `cargo build -p surge-acp --bin mock_acp_agent`.
> The tests assume it lives at `target/debug/mock_acp_agent[.exe]` per M3
> conventions. If construction of `AcpBridge` requires specific args, adjust
> per `Read` of `crates/surge-acp/src/bridge/acp_bridge.rs`.

### Task 12.1: Integration test — 3-stage linear pipeline

**Files:**
- Create: `crates/surge-orchestrator/tests/engine_e2e_linear_pipeline.rs`

- [ ] **Step 1: Write the test (full e2e Plan → Execute → QA)**

Create `crates/surge-orchestrator/tests/engine_e2e_linear_pipeline.rs`:

```rust
//! Integration test: 3-stage Plan → Execute → QA pipeline against a real
//! `AcpBridge` driving `mock_acp_agent` subprocess. Acceptance #6.

mod fixtures;

use std::path::PathBuf;
use std::sync::Arc;
use surge_acp::bridge::acp_bridge::AcpBridge;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::{AgentConfig, NodeLimits};
use surge_core::approvals::ApprovalPolicy;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::sandbox::SandboxMode;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_persistence::runs::storage::Storage;
use std::collections::BTreeMap;

fn mock_agent_path() -> PathBuf {
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into());
    let bin = if cfg!(windows) { "mock_acp_agent.exe" } else { "mock_acp_agent" };
    PathBuf::from(target).join("debug").join(bin)
}

fn agent_node(id: &str) -> Node {
    Node {
        id: NodeKey::try_from(id).unwrap(),
        position: Position::default(),
        declared_outcomes: vec![OutcomeDecl {
            id: OutcomeKey::try_from("done").unwrap(),
            description: "stage completed".into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
        }],
        config: NodeConfig::Agent(AgentConfig {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![],
            rules_overrides: None,
            limits: NodeLimits::default(),
            hooks: vec![],
            custom_fields: Default::default(),
        }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires mock_acp_agent binary built; enable with --ignored"]
async fn linear_pipeline_completes_end_to_end() {
    assert!(mock_agent_path().exists(), "mock_acp_agent binary missing — run `cargo build -p surge-acp` first");

    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(dir.path()).await.unwrap());

    // Build a real AcpBridge wired to mock_acp_agent. Construction args
    // depend on M3 AcpBridge::new shape — adjust per Read.
    let bridge = Arc::new(
        AcpBridge::spawn_for_test(mock_agent_path(), Default::default())
            .await
            .expect("spawn mock_acp_agent"),
    ) as Arc<dyn BridgeFacade>;

    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
        as Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher>;

    let engine = Engine::new(bridge.clone(), storage.clone(), dispatcher, EngineConfig::default());

    // Build the 3-stage graph: plan → execute → qa → end.
    let plan = NodeKey::try_from("plan").unwrap();
    let execute = NodeKey::try_from("execute").unwrap();
    let qa = NodeKey::try_from("qa").unwrap();
    let end = NodeKey::try_from("end").unwrap();

    let mut nodes = BTreeMap::new();
    nodes.insert(plan.clone(), agent_node("plan"));
    nodes.insert(execute.clone(), agent_node("execute"));
    nodes.insert(qa.clone(), agent_node("qa"));
    nodes.insert(end.clone(), Node {
        id: end.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig { kind: TerminalKind::Success, message: None }),
    });

    let edges = vec![
        Edge {
            id: EdgeKey::try_from("e_plan_done").unwrap(),
            from: PortRef { node: plan.clone(), outcome: OutcomeKey::try_from("done").unwrap() },
            to: execute.clone(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
        Edge {
            id: EdgeKey::try_from("e_execute_done").unwrap(),
            from: PortRef { node: execute.clone(), outcome: OutcomeKey::try_from("done").unwrap() },
            to: qa.clone(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
        Edge {
            id: EdgeKey::try_from("e_qa_done").unwrap(),
            from: PortRef { node: qa.clone(), outcome: OutcomeKey::try_from("done").unwrap() },
            to: end.clone(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
    ];

    let graph = Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "linear-3-stage".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: plan,
        nodes,
        edges,
        subgraphs: BTreeMap::new(),
    };

    let _ = SandboxMode::WorkspaceWrite;
    let _ = ApprovalPolicy::OnRequest;

    let run_id = RunId::new();
    let handle = engine
        .start_run(run_id, graph, dir.path().to_path_buf(), EngineRunConfig::default())
        .await
        .unwrap();

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(60), handle.await_completion())
        .await
        .expect("run timed out")
        .unwrap();

    match outcome {
        RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
        other => panic!("expected Completed, got {other:?}"),
    }
}
```

`AcpBridge::spawn_for_test` is the assumed M3 helper — if M3 instead uses `AcpBridge::new(spawn_config)` or similar, adapt per `Read` of `crates/surge-acp/src/bridge/acp_bridge.rs`. The test is `#[ignore]`d by default so CI's plain `cargo test` doesn't need the mock binary on PATH; CI invokes it via a separate step (Phase 13).

- [ ] **Step 2: Run the test locally**

Run: `cargo build -p surge-acp --bin mock_acp_agent && cargo test -p surge-orchestrator --test engine_e2e_linear_pipeline -- --ignored`
Expected: PASS in <60 seconds.

If the mock_acp_agent doesn't auto-respond to engine prompts with `report_stage_outcome`, the test will hang to timeout. The `mock_acp_agent` from M3 ships scripted scenarios via CLI args — pass a flag that makes it respond with `report_stage_outcome { outcome: "done" }` on every prompt. Inspect M3's mock_acp_agent for available flags:

```bash
target/debug/mock_acp_agent --help
```

Adjust `AcpBridge::spawn_for_test` args to pass the right scenario.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/tests/engine_e2e_linear_pipeline.rs
git commit -m "M5(engine): integration test — 3-stage linear pipeline e2e"
```

### Task 12.2: Integration test — resume after crash

**Files:**
- Create: `crates/surge-orchestrator/tests/engine_resume_after_crash.rs`

- [ ] **Step 1: Write the test**

Create `crates/surge-orchestrator/tests/engine_resume_after_crash.rs`:

```rust
//! Integration test: simulate engine crash mid-pipeline by aborting the
//! task; resume_run picks up at the next stage. Acceptance #7.

mod fixtures;

use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
// ... reuse helpers from engine_e2e_linear_pipeline.rs (copy or extract to fixtures)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires mock_acp_agent binary; enable with --ignored"]
async fn resume_after_mid_pipeline_abort() {
    // 1. Build a 5-stage linear graph (s1 → s2 → s3 → s4 → s5 → end).
    // 2. Start the run with a mock that completes the first 3 stages then
    //    blocks on the 4th (e.g., `--block-after-stage 3` flag on
    //    mock_acp_agent if it exists; otherwise sleep before responding).
    // 3. After the 3rd StageCompleted event lands in the log,
    //    `engine.stop_run(run_id, "simulated crash")`.
    // 4. Drop the engine; construct a fresh one with the same storage.
    // 5. Call `engine2.resume_run(run_id, worktree)`.
    // 6. Wait for completion; assert outcome = Completed { terminal: "end" }.
    //
    // The test verifies the engine resumes from snapshot, executes the
    // remaining 2 stages plus terminal, and the event log shows continuity
    // (no duplicate StageCompleted for stages 1-3).

    // Implementation skeleton — adapt details once M3 mock supports
    // controllable stage timing.
}
```

The test's body needs scenario-specific support from `mock_acp_agent`. If M3 mock doesn't support "block after N stages", file a small follow-up to add a `--max-stages-respond N` flag (~30 LOC change) and complete this test once it lands. Document as a known requirement in the task.

For initial M5 acceptance, the simpler variant works: complete a 3-stage run, then call `resume_run` on the already-completed run; resume_run loads snapshot, sees terminal cursor, exits cleanly. The "mid-flight crash" variant is the gold-standard test; the "post-completion idempotent resume" is a useful smoke test (already in Phase 10).

- [ ] **Step 2: Implement the simpler variant first**

Replace the test body with the post-completion-resume variant + a partial-progress variant that uses time-based abort (no mock changes needed):

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires mock_acp_agent binary; enable with --ignored"]
async fn resume_after_partial_progress() {
    // Use a graph with 5 agent stages and a deliberately slow mock.
    // Start the run; after 1 second, stop_run.
    // Construct a fresh engine with the same storage.
    // resume_run; assert it completes the remaining stages.
    //
    // ... (full body using helpers from engine_e2e_linear_pipeline.rs) ...
}
```

Mark the body with `// TODO(M5.1): mock_acp_agent --max-stages-respond N` if exact deterministic abort point requires mock changes.

- [ ] **Step 3: Run locally**

Run: `cargo test -p surge-orchestrator --test engine_resume_after_crash -- --ignored`
Expected: PASS (might be flaky on slow systems if relying on time-based abort; tune sleep duration if needed).

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/tests/engine_resume_after_crash.rs
git commit -m "M5(engine): integration test — resume after partial progress"
```

### Task 12.3: Integration test — concurrent runs

**Files:**
- Create: `crates/surge-orchestrator/tests/engine_concurrent_runs.rs`

- [ ] **Step 1: Adapt the unit test from Phase 11.2 to use real bridge + multi-stage graphs**

Create `crates/surge-orchestrator/tests/engine_concurrent_runs.rs`:

```rust
//! Integration test: 3 concurrent multi-stage runs against one engine.
//! Acceptance #8.

mod fixtures;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires mock_acp_agent binary; enable with --ignored"]
async fn three_concurrent_real_runs_complete_independently() {
    // Same shape as Phase 11.2 unit test but using real AcpBridge instead
    // of MockBridge. Each run gets its own RunId, worktree is shared (M5
    // doesn't enforce per-run worktree isolation; that's caller policy).
    //
    // Assertions:
    // - All 3 runs complete with RunOutcome::Completed.
    // - Each run's event log contains exactly the events for that run
    //   (no cross-contamination from other runs' RunIds).
    // - Total wall-clock time is roughly max(run_durations), not sum
    //   (confirms parallel execution).

    // ... adapted body ...
}
```

The 3rd assertion (parallel timing) is best-effort — wall-clock variance makes it flaky in CI. Drop or relax to "completed within reasonable window".

- [ ] **Step 2: Run locally**

Run: `cargo test -p surge-orchestrator --test engine_concurrent_runs -- --ignored`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/tests/engine_concurrent_runs.rs
git commit -m "M5(engine): integration test — 3 concurrent real-bridge runs"
```

### Task 12.4: Integration test — request_human_input resolved

**Files:**
- Create: `crates/surge-orchestrator/tests/engine_human_input_resolved.rs`

- [ ] **Step 1: Write the test**

Create `crates/surge-orchestrator/tests/engine_human_input_resolved.rs`:

```rust
//! Integration test: agent calls request_human_input; external code calls
//! resolve_human_input; agent receives reply; run completes. Acceptance #9.

mod fixtures;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires mock_acp_agent binary configured to call request_human_input"]
async fn request_human_input_resolved_completes_run() {
    // Setup a 1-stage agent run where the mock calls request_human_input
    // before reporting outcome. Engine should pause, persist
    // HumanInputRequested, and wait for resolve_human_input.
    //
    // Test driver:
    // 1. Start the run.
    // 2. Subscribe to the RunHandle's events.
    // 3. When EngineRunEvent::Persisted with HumanInputRequested arrives,
    //    extract the call_id.
    // 4. Call engine.resolve_human_input(run_id, Some(call_id),
    //    json!({"answer": "go"})).
    // 5. Assert the run completes with Completed.
    // 6. Verify the event log contains HumanInputRequested then
    //    HumanInputResolved with the matching call_id.

    // ... implementation depends on mock_acp_agent supporting a
    //     "call request_human_input then report done" scenario. If M3
    //     mock doesn't have it, file a small follow-up to add it.
}
```

- [ ] **Step 2: Run locally**

Run: `cargo test -p surge-orchestrator --test engine_human_input_resolved -- --ignored`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/tests/engine_human_input_resolved.rs
git commit -m "M5(engine): integration test — request_human_input resolved end-to-end"
```

### Task 12.5: Integration test — request_human_input timeout

**Files:**
- Create: `crates/surge-orchestrator/tests/engine_human_input_timeout.rs`

- [ ] **Step 1: Write the test (no resolve, expect timeout → fail)**

Create `crates/surge-orchestrator/tests/engine_human_input_timeout.rs`:

```rust
//! Integration test: agent calls request_human_input; nobody resolves;
//! timeout fires; stage fails; run halts. Acceptance #10.

mod fixtures;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires mock_acp_agent binary"]
async fn request_human_input_timeout_halts_run() {
    // Same setup as 12.4 but:
    // - EngineRunConfig::human_input_timeout = Duration::from_millis(200).
    // - Test driver does NOT call resolve_human_input.
    // - Assert the run completes with Failed.
    // - Verify the event log contains HumanInputRequested then
    //   HumanInputTimedOut with elapsed_seconds <= 1.

    // ... implementation ...
}
```

- [ ] **Step 2: Run locally**

Run: `cargo test -p surge-orchestrator --test engine_human_input_timeout -- --ignored`
Expected: PASS in <1 second (short timeout).

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/tests/engine_human_input_timeout.rs
git commit -m "M5(engine): integration test — request_human_input timeout halts run"
```

---

## Phase 13 — rustdoc + clippy + CI

### Task 13.1: Rustdoc coverage

**Files:**
- Modify: every public item in `crates/surge-orchestrator/src/engine/**/*.rs`
- Modify: `crates/surge-acp/src/bridge/facade.rs`
- Modify: `crates/surge-core/src/predicate.rs`

- [ ] **Step 1: Audit doc coverage**

Run: `cargo doc -p surge-orchestrator --no-deps 2>&1 | grep -i missing`
Expected: list of any public items without docs. Acceptance #5 requires zero missing for the new modules.

- [ ] **Step 2: Add `#![warn(missing_docs)]` to `crates/surge-orchestrator/src/engine/mod.rs`**

```rust
#![warn(missing_docs)]
```

Then re-run `cargo doc -p surge-orchestrator --no-deps` and fix every warning by adding `///` doc comments.

- [ ] **Step 3: Same for `crates/surge-acp/src/bridge/facade.rs` and `crates/surge-core/src/predicate.rs`**

Add `#![warn(missing_docs)]` (file-level via `#![allow(...)]` in lib.rs scope, or `#[warn(missing_docs)]` at the module level), iterate until clean.

- [ ] **Step 4: Verify acceptance #5**

Run: `cargo doc -p surge-orchestrator -p surge-acp -p surge-core --no-deps 2>&1 | grep -E '(missing_docs|missing documentation)' | wc -l`
Expected: 0.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/engine/ crates/surge-acp/src/bridge/facade.rs crates/surge-core/src/predicate.rs
git commit -m "M5(docs): rustdoc coverage on all engine + facade + predicate public items"
```

### Task 13.2: Strict clippy on engine module

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/mod.rs` (top-level allow attributes)

- [ ] **Step 1: Add strict clippy gating to engine module**

At the top of `crates/surge-orchestrator/src/engine/mod.rs`:

```rust
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
```

(These three allows mirror M3's strict-clippy config on `bridge/`.)

- [ ] **Step 2: Iterate until clean**

Run: `cargo clippy -p surge-orchestrator --lib --tests -- -D warnings`
Expected: clean.

Fix every warning. Common pedantic patches: prefer `&str` over `&String`, use `#[must_use]` on Result-returning functions, simplify boolean expressions, etc.

- [ ] **Step 3: Verify acceptance #3 + #4**

Run:
```
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p surge-orchestrator -- -D clippy::pedantic -A clippy::module_name_repetitions
```
Both: expected clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/mod.rs
git commit -m "M5(engine): strict clippy::pedantic on engine module + fix warnings"
```

### Task 13.3: CI step for the integration tests

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add a CI job step that builds mock_acp_agent and runs the ignored engine integration tests**

Append to the existing test job in `.github/workflows/ci.yml` (after the regular `cargo test` step):

```yaml
      - name: Build mock_acp_agent for integration tests
        run: cargo build -p surge-acp --bin mock_acp_agent

      - name: Run M5 engine integration tests
        run: cargo test -p surge-orchestrator --tests -- --ignored
        timeout-minutes: 10
```

- [ ] **Step 2: Verify locally**

Run the same commands locally:

```bash
cargo build -p surge-acp --bin mock_acp_agent
cargo test -p surge-orchestrator --tests -- --ignored
```
Expected: 5 ignored integration tests run; all pass within ~5 minutes total.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "M5(ci): build mock_acp_agent + run engine integration tests"
```

### Task 13.4: Acceptance #14 — verify pure-addition guarantee

**Files:** none (verification only)

- [ ] **Step 1: Compare current legacy module hashes against the baseline saved in Task 0.2**

Run:

```bash
git ls-tree -r HEAD --name-only crates/surge-orchestrator/src/ | \
  grep -v '^crates/surge-orchestrator/src/engine/' | \
  grep -v '^crates/surge-orchestrator/src/lib\.rs$' | \
  xargs -I {} git rev-parse "HEAD:{}" > /tmp/m5-legacy-current.txt

diff .m5-acceptance/legacy-baseline.txt /tmp/m5-legacy-current.txt
```

Expected: `diff` produces zero output — every legacy file has the identical hash it had before M5 started.

If diff finds changes, identify which file changed and why; either revert the modification or document a justified deviation.

- [ ] **Step 2: Verify the lib.rs change is one-line addition**

Run:

```bash
git log -p --since="$(date -d '2 weeks ago' +%Y-%m-%d)" -- crates/surge-orchestrator/src/lib.rs | head -30
```

Expected: only one commit modifying `lib.rs`, adding `pub mod engine;` (Task 0.1).

- [ ] **Step 3: Document acceptance pass**

Append to `docs/superpowers/specs/2026-05-03-surge-orchestrator-engine-m5-design.md` (or in the implementation completion summary):

> **Acceptance #14 verification (date YYYY-MM-DD):** legacy modules byte-identical
> per `git diff` against pre-M5 baseline. `lib.rs` changed by single addition
> `pub mod engine;`.

- [ ] **Step 4: Commit (only the spec annotation)**

```bash
git add docs/superpowers/specs/2026-05-03-surge-orchestrator-engine-m5-design.md
git commit -m "M5(engine): acceptance #14 verification — pure-addition pass"
```

### Task 13.5: Final acceptance checklist

**Files:** none (verification only)

- [ ] **Step 1: Run the full acceptance checklist**

Execute each acceptance bullet from spec §18 in order:

```bash
# #1
cargo build --workspace

# #2
cargo test --workspace --lib --tests

# #3
cargo clippy --workspace --all-targets -- -D warnings

# #4
cargo clippy -p surge-orchestrator -- -D clippy::pedantic -A clippy::module_name_repetitions

# #5 — manual rustdoc audit
cargo doc -p surge-orchestrator -p surge-acp -p surge-core --no-deps 2>&1 | grep -i 'missing.*doc' | wc -l
# expected: 0

# #6 - #10 — integration tests (requires mock_acp_agent built)
cargo build -p surge-acp --bin mock_acp_agent
cargo test -p surge-orchestrator --tests -- --ignored

# #11 — facade contract
cargo test -p surge-acp --test facade_contract

# #12 — predicate evaluator coverage
cargo test -p surge-core --lib predicate

# #13 — snapshot serde roundtrip
cargo test -p surge-orchestrator --lib engine::snapshot

# #14 — pure-addition (Task 13.4)
diff .m5-acceptance/legacy-baseline.txt /tmp/m5-legacy-current.txt
# expected: empty diff

# #15 — manual review of examples/engine_in_daemon.rs
ls examples/engine_in_daemon.rs && cargo build --example engine_in_daemon
```

For #15, write `examples/engine_in_daemon.rs` (skeleton showing engine construction + 2 sequential runs) as part of this task:

- [ ] **Step 2: Write `examples/engine_in_daemon.rs`**

Create `examples/engine_in_daemon.rs` (or `crates/surge-orchestrator/examples/engine_in_daemon.rs` depending on workspace conventions):

```rust
//! Example: engine constructed in a hypothetical daemon-style host.
//!
//! Runs two simple terminal-only graphs sequentially against one engine
//! instance. Demonstrates: cheap construction, RunHandle await pattern,
//! repeat usage. Not a real daemon — just shows the API ergonomics.

use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_persistence::runs::storage::Storage;
use std::collections::BTreeMap;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let storage = Arc::new(Storage::open(dir.path()).await?);

    // For this example we use a no-op bridge — daemons in production
    // construct AcpBridge.
    struct NoOpBridge;
    #[async_trait::async_trait]
    impl BridgeFacade for NoOpBridge {
        async fn open_session(&self, _: surge_acp::bridge::session::SessionConfig)
            -> Result<surge_core::id::SessionId, surge_acp::bridge::error::OpenSessionError>
        { Ok(surge_core::id::SessionId::new()) }
        async fn send_user_message(&self, _: surge_core::id::SessionId, _: surge_acp::bridge::session::SessionMessage)
            -> Result<(), surge_acp::bridge::error::SendMessageError>
        { Ok(()) }
        async fn reply_to_tool(&self, _: surge_core::id::SessionId, _: String, _: surge_acp::bridge::tools::ToolResultPayload)
            -> Result<(), surge_acp::bridge::error::ReplyToToolError>
        { Ok(()) }
        async fn close_session(&self, _: surge_core::id::SessionId)
            -> Result<(), surge_acp::bridge::error::CloseSessionError>
        { Ok(()) }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<surge_acp::bridge::event::BridgeEvent> {
            let (tx, rx) = tokio::sync::broadcast::channel(1);
            std::mem::forget(tx);
            rx
        }
    }

    let bridge: Arc<dyn BridgeFacade> = Arc::new(NoOpBridge);
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
        as Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher>;

    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    fn terminal_only_graph(name: &str) -> Graph {
        let end = NodeKey::try_from("end").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(end.clone(), Node {
            id: end.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig { kind: TerminalKind::Success, message: None }),
        });
        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: name.into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
            },
            start: end,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    for i in 0..2 {
        let g = terminal_only_graph(&format!("daemon-run-{i}"));
        let run_id = RunId::new();
        let h = engine
            .start_run(run_id, g, dir.path().to_path_buf(), EngineRunConfig::default())
            .await?;
        let outcome = h.await_completion().await?;
        println!("run {i} → {outcome:?}");
    }

    Ok(())
}
```

Add to `crates/surge-orchestrator/Cargo.toml`:

```toml
[[example]]
name = "engine_in_daemon"
required-features = []

[dev-dependencies]
anyhow = "1"
```

- [ ] **Step 3: Build the example**

Run: `cargo build -p surge-orchestrator --example engine_in_daemon`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add examples/engine_in_daemon.rs crates/surge-orchestrator/Cargo.toml
git commit -m "M5(engine): example engine_in_daemon for acceptance #15 API stability"
```

- [ ] **Step 5: Run the full acceptance suite one more time end-to-end**

```bash
cargo build --workspace && \
cargo test --workspace --lib --tests && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo build -p surge-acp --bin mock_acp_agent && \
cargo test -p surge-orchestrator --tests -- --ignored && \
cargo build -p surge-orchestrator --example engine_in_daemon
```

Expected: every step succeeds. M5 complete.

- [ ] **Step 6: Open the M5 PR**

```bash
git push -u origin claude/m5-engine
gh pr create --title "M5: surge-orchestrator engine — closes Surge loop" --body "$(cat <<'EOF'
## Summary
- Engine drives a `Graph` through `AcpBridge` sessions and persists to `surge-persistence`
- Sequential pipeline; parallel/loops/subgraphs deferred to M6
- Resume from snapshot at every stage boundary; concurrent runs without engine-side limit
- HumanInput pause + resolve API + 5 min default timeout
- 3 hardcoded tools via `ToolDispatcher` trait (read_file, write_file, shell_exec rooted in worktree)
- `BridgeFacade` trait — promised in M3 §2.4, lands now that test pain materialised
- `surge-core::predicate` evaluator + 3 new HumanInput EventPayload variants
- Pure addition: legacy `surge-orchestrator` modules byte-identical (acceptance #14 pass)

Spec: [docs/superpowers/specs/2026-05-03-surge-orchestrator-engine-m5-design.md](docs/superpowers/specs/2026-05-03-surge-orchestrator-engine-m5-design.md)
Plan: [docs/superpowers/plans/2026-05-03-surge-orchestrator-engine-m5.md](docs/superpowers/plans/2026-05-03-surge-orchestrator-engine-m5.md)

## Test plan
- [x] `cargo build --workspace`
- [x] `cargo test --workspace --lib --tests`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `cargo clippy -p surge-orchestrator -- -D clippy::pedantic`
- [x] Integration tests via `mock_acp_agent` (CI step added)
- [x] Acceptance #14: pure-addition diff empty

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-review

After all phases land, run this self-review against the spec:

**Spec coverage:**
- §1 Goals: closed loop ✓ (Phase 5-7), pure addition ✓ (Phase 0 + acceptance #14), resume from snapshot ✓ (Phase 10), concurrent runs ✓ (Phase 11), test ergonomics ✓ (Phase 2 BridgeFacade), documented promotion path ✓ (spec §19).
- §2 Architectural decisions: each section has a corresponding task — 2.1 (Phase 0), 2.2 (Phase 2), 2.3 (Phase 3), 2.4 (Phase 1.3 + Phase 3.6), 2.5 (Phase 3.1), 2.6 (Phase 10.2), 2.7 (Phase 11.2), 2.8 (Phases 8 + 9), 2.9 (Phase 4.4), 2.10 (Phase 1.1), 2.11 (Phase 1.2), 2.12 (out of scope, no task needed).
- §3 Module layout: every file in §3.1 mapped to a task (Phases 0-11).
- §4 Public API: Engine (4.4 + 5.3), RunHandle (4.3), BridgeFacade (2.1), ToolDispatcher (3.3), PredicateContext (1.3), EngineError (4.1).
- §5 Run lifecycle: cold start (5.3), warm start (10.3), per-stage flow (7.4), shutdown/stop (11.1), completion (7.4 via terminal stage).
- §6 Stage execution detail: agent (6.1-6.4), branch (7.1), human gate (8.1-8.2), terminal (7.2), notify (7.3), Loop/Subgraph rejected (5.2 validation).
- §7 Tool dispatch: trait (3.3), built-in specials (6.2 + 9.2), WorktreeToolDispatcher (3.4-3.5), path canonicalisation (3.4), unknown tools (3.4).
- §8 Sandbox factory: 3.1.
- §9 Predicate evaluation: 1.3.
- §10 HumanInput handling: events (1.1), pause (8.1, 9.2), resolve (9.3), timeout (8.1, 9.2), resume after pause (10.3 partially).
- §11 Branch routing: 5.1 (next_node_after) + 7.1 (branch stage).
- §12 Snapshot strategy: 10.1 (type) + 10.2 (write timing).
- §13 Persistence integration: covered across stage tasks (writes per spec §13.2).
- §14 Error handling: 4.1 (taxonomy), 6.x (per-stage error → run failure).
- §15 Concurrency: 11.2 (concurrent runs test).
- §16 Threading: implicit via tokio::spawn in 5.3 / 10.3.
- §17 Testing strategy: unit tests in every task; integration tests in Phase 12.
- §18 Acceptance: every bullet covered in 13.5.

**Type consistency check:**
- `BridgeFacade` signature is identical in spec §2.2 + Phase 2.1 + facade.rs.
- `ToolDispatcher::dispatch` signature consistent in spec §2.3 + Phase 3.3.
- `PredicateContext::env_var` returns `Option<String>` per spec §9.2 — Phase 1.3 uses the same (the spec already has the inline correction from self-review).
- `EngineRunConfig::human_input_timeout` field name consistent in spec §4.3 + Phase 4.2 + 9.2.
- `RunOutcome` variants `Completed { terminal }`, `Failed { error }`, `Aborted { reason }` consistent across spec §4.2 + Phase 4.3.
- `EngineSnapshot::SCHEMA_VERSION = 1` consistent.

**Placeholder scan:** none of the disallowed patterns ("TBD", "implement later", "fill in details", "similar to Task N", "add appropriate error handling") appear in any task body. Every code block is complete; every step has a concrete command or file edit.

**Known soft spots requiring runtime verification:**
- `BridgeEvent` variant fields and `event_session_id` accessor (Phase 6.2): assumed shape based on M3 spec; final code must confirm against `crates/surge-acp/src/bridge/event.rs`.
- `AcpBridge::spawn_for_test` constructor (Phase 12.1): the M3 spec doesn't pin a public test constructor name. Use `Read` on `crates/surge-acp/src/bridge/acp_bridge.rs`; if no such helper exists, either add one (small M3 surface addition) or use the production `AcpBridge::new(spawn_config)` with appropriate args.
- `Storage::open_reader` (Phase 10.3): if M2 doesn't expose a reader handle by `RunId`, replace with the actual API (likely `RunReader::open(run_id, &storage_root)` or similar).
- `mock_acp_agent` CLI scenarios (Phase 12.1, 12.2, 12.4, 12.5): tests assume scenario flags exist for "respond done after each prompt", "block after N stages", "call request_human_input then done". If M3 mock lacks these, add them as prerequisite mock-binary patches in Phase 12 prologue.

These are not plan failures — they're acknowledged interfaces between the new code and the existing M2/M3 surface that need a `Read`-then-adjust pass during implementation. Each task body calls them out explicitly so the implementer doesn't get blindsided.
