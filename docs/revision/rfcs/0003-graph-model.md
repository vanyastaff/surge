# RFC-0003 · Graph Model

## Overview

This document specifies the data model for pipelines (graphs), the seven node types, edge semantics, and validation rules. The graph model is the user-facing abstraction — what you see on the canvas, what is serialized to `flow.toml`.

## Core types

### Graph

```rust
struct Graph {
    schema_version: u32,
    nodes: BTreeMap<NodeId, Node>,
    edges: Vec<Edge>,
    start: NodeId,
    metadata: GraphMetadata,
}

struct GraphMetadata {
    name: String,
    description: Option<String>,
    template_origin: Option<TemplateRef>,  // if generated from a template
    created_at: DateTime<Utc>,
    author: Option<String>,
}
```

`NodeId` is a typed key — `Key<NodeDomain>` from `domain-key` crate. Stable string IDs (e.g., `impl_2`, `review_main`).

### Node

```rust
struct Node {
    id: NodeId,
    kind: NodeKind,                    // closed enum
    position: Position,                // canvas coords (x, y)
    config: NodeConfig,                // type-erased per-kind config
    profile_ref: Option<ProfileRef>,   // for Agent nodes
    declared_outcomes: Vec<OutcomeDecl>,
}

struct Position { x: f32, y: f32 }

struct OutcomeDecl {
    id: OutcomeId,                     // stable, e.g. "done", "blocked"
    description: String,               // human-readable
    edge_kind_hint: EdgeKind,          // forward / backtrack / escalate
    is_terminal: bool,                 // does this end the run?
}
```

Each declared outcome creates an output port on the canvas. Engine validates that every declared outcome has at most one outgoing edge (or zero, in which case reaching it ends the stage with no routing target — usually means user forgot to wire it).

### NodeKind

```rust
enum NodeKind {
    Agent,
    HumanGate,
    Branch,
    Terminal,
    Notify,
    Loop,
    Subgraph,
}
```

**Closed enum.** Adding a new variant requires modification of the core crate. This is intentional:
- Compiler enforces exhaustive match in executor
- Static validation at TOML load
- Plugin systems for custom nodes are out of scope for v1.0

The extensibility mechanism is **profiles for Agent nodes**, not new node kinds.

### Edge

```rust
struct Edge {
    id: EdgeId,
    from: PortRef,
    to: NodeId,
    kind: EdgeKind,
    policy: EdgePolicy,
}

struct PortRef {
    node: NodeId,
    outcome: OutcomeId,                // which port on source
}

enum EdgeKind {
    Forward,                           // standard progression
    Backtrack,                         // routes to earlier node (creates cycle)
    Escalate,                          // routes to HumanGate or notification
}

struct EdgePolicy {
    max_traversals: Option<u32>,       // for Backtrack: limit cycles
    on_max_exceeded: ExceededAction,   // Escalate | Fail
    label: Option<String>,             // optional display label
}
```

Engine enforces `max_traversals` to prevent runaway cycles. Default for `Backtrack`: `max_traversals: 3, on_max_exceeded: Escalate`.

## Node types in detail

### Agent

The most common node. Runs an ACP session to do agent work.

```rust
struct AgentConfig {
    profile: ProfileRef,               // mandatory — references registry
    prompt_overrides: Option<PromptOverride>,
    tool_overrides: Option<ToolOverride>,
    sandbox_override: Option<SandboxConfig>,
    bindings: Vec<Binding>,            // input artifacts
    rules_overrides: Option<RulesOverride>,
    limits: NodeLimits,
    hooks: Vec<Hook>,
}

struct Binding {
    source: ArtifactSource,            // where the data comes from
    target: TemplateVar,               // {{spec}}, {{plan}}, etc.
}

enum ArtifactSource {
    NodeOutput { node: NodeId, artifact: String },  // e.g. plan_1.plan.md
    RunArtifact { name: String },                    // e.g. __run.description.md
    GlobPattern { node: NodeId, pattern: String },   // multiple files matching
    Static { content: String },                      // hardcoded value
}

struct NodeLimits {
    timeout_seconds: u32,              // default 900 (15 min)
    max_retries: u32,                  // default 3
    circuit_breaker: Option<CbConfig>,
    max_tokens: u32,                   // default 200_000
}
```

**Outcomes** — declared per-node, common patterns:
- `done` (success, forward) + `blocked` (backtrack to plan) + `escalate` (HumanGate)
- `pass` (forward) + `fail` (backtrack to implement) — for verifiers/reviewers
- `done` (forward) + `unclear` (HumanGate) — for analysts

The agent **must** call `report_stage_outcome` tool with one of declared outcome IDs as final action. Failure to do so within timeout = node failure.

### HumanGate

Pauses execution awaiting human decision.

```rust
struct HumanGateConfig {
    channels: Vec<ApprovalChannel>,    // [Telegram, UI, Email] in priority order
    timeout_seconds: Option<u32>,      // None = wait forever
    on_timeout: TimeoutAction,         // Reject | Escalate | Continue
    summary: SummaryTemplate,          // what to show user
    options: Vec<ApprovalOption>,      // outcomes user can pick
    allow_freetext: bool,              // can user reply with comment?
}

struct ApprovalOption {
    outcome: OutcomeId,                // maps to outcome on this node
    label: String,                     // "Approve", "Reject", etc.
    style: OptionStyle,                // primary | danger | warn | normal
}

struct SummaryTemplate {
    title: String,
    body: String,                      // template with {{vars}}
    show_artifacts: Vec<ArtifactSource>, // which artifacts to embed
}
```

**Outcomes** — exactly one per `ApprovalOption`. User's choice routes the edge. Free-text reply goes into a `human_comment.md` artifact for next stage.

### Branch

Conditional routing without LLM. Pure predicates.

```rust
struct BranchConfig {
    predicates: Vec<BranchArm>,
    default_outcome: OutcomeId,        // if no arm matches
}

struct BranchArm {
    condition: Predicate,
    outcome: OutcomeId,
}

enum Predicate {
    FileExists { path: String },
    ArtifactSize { artifact: String, op: CompareOp, value: u64 },
    OutcomeMatches { node: NodeId, outcome: OutcomeId },
    EnvVar { name: String, op: CompareOp, value: String },
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Not(Box<Predicate>),
}
```

Predicates are evaluated against `RunMemory` (cumulative state of the run). No LLM call, instant. Useful for:
- Detecting project type before agent stages run
- Skipping optional stages (e.g., skip Reviewer for trivial changes)
- Routing based on previous outcome (post-mortem branching)

**Outcomes** — declared explicitly, must include `default_outcome`.

### Terminal

End-of-run marker. Has no outgoing edges.

```rust
struct TerminalConfig {
    kind: TerminalKind,
    message: Option<String>,
}

enum TerminalKind {
    Success,                           // RunCompleted
    Failure { exit_code: i32 },        // RunFailed
    Aborted,                           // RunAborted (user-initiated)
}
```

Reaching a Terminal node writes the corresponding terminal event and ends the run. Multiple Terminal nodes per graph allowed (e.g., `success_pr`, `aborted_human`, `failed_review`).

### Notify

Side-effect node: sends notification, doesn't pause.

```rust
struct NotifyConfig {
    channel: NotificationChannel,      // Telegram | Slack | Email | Desktop | Webhook
    template: NotifyTemplate,
    on_failure: NotifyFailureAction,   // Continue | Fail
}

struct NotifyTemplate {
    severity: NotifySeverity,          // Info | Warn | Error | Success
    title: String,
    body: String,
    artifacts: Vec<ArtifactSource>,    // optional inline content
}
```

**Outcomes** — single `sent` outcome (forward). On failure: configurable.

### Loop

Iterates a body subgraph over a collection.

```rust
struct LoopConfig {
    iterates_over: IterableSource,
    body: Box<Graph>,                  // inline subgraph
    iteration_var_name: String,        // e.g. "milestone" or "task"
    exit_condition: ExitCondition,
    on_iteration_failure: FailurePolicy,
    parallelism: ParallelismMode,      // v1: Sequential only
    gate_after_each: bool,             // optional HumanGate between iterations
}

enum IterableSource {
    Artifact { node: NodeId, name: String, jsonpath: String },
    // e.g. roadmap.md → milestones[]
    Static(Vec<Value>),
}

enum ExitCondition {
    AllItems,                          // process all items
    UntilOutcome { from_node: NodeId, outcome: OutcomeId },
    MaxIterations { n: u32 },
}

enum FailurePolicy {
    Abort,                             // stop loop, fail parent
    Skip,                              // skip failed item, continue
    Retry { max: u32 },                // retry item N times
    Replan,                            // back to outer plan stage
}
```

**Outcomes**:
- `completed` — all iterations done
- `aborted` — failure policy triggered abort
- `gate_decision` — if gate_after_each, user can choose between iterations

The body subgraph receives `iteration_var_name` (e.g., `{{milestone}}`) as a binding available to its nodes.

### Subgraph

Encapsulates a reusable inner graph as a single node on the outer canvas.

```rust
struct SubgraphConfig {
    inner: Box<Graph>,
    inputs: Vec<SubgraphInput>,        // outer→inner mappings
    outputs: Vec<SubgraphOutput>,      // inner→outer mappings
}

struct SubgraphInput {
    outer_binding: Binding,
    inner_var: TemplateVar,            // available inside subgraph
}

struct SubgraphOutput {
    inner_artifact: ArtifactSource,    // produced by inner node
    outer_outcome: OutcomeId,          // exposed on outer node port
}
```

Subgraphs are a v1 feature (not v2 as previously discussed) because Loop bodies are subgraphs.

## Outcome semantics

Outcomes are the **typed contract** between nodes. The defining principle:

> An agent node returns to the engine via `report_stage_outcome`. The outcome ID must be one of the node's declared outcomes. The engine matches the outcome ID to a specific edge and routes accordingly. There is no ambiguity, no "what now" — every declared outcome has a deterministic next destination.

### Outcome IDs

Stable string identifiers, conventionally lowercase with underscores. Standard outcomes:
- `done` — success forward path
- `pass` / `fail` — for verifiers
- `blocked` — agent cannot proceed, needs replanning
- `escalate` — needs human decision
- `unclear` — ambiguity in inputs, needs clarification

Custom outcomes for specific node types: `arch_issue`, `logic_error`, `nitpicks_only`, etc.

### Outcome → Edge resolution

```rust
fn resolve_next(graph: &Graph, current: NodeId, outcome: OutcomeId) -> Result<NodeId> {
    let port = PortRef { node: current, outcome };
    let edge = graph.edges.iter()
        .find(|e| e.from == port)
        .ok_or(EngineError::UndeclaredOutcomeRoute)?;
    Ok(edge.to)
}
```

If an outcome has no edge: that's a graph validation error caught at load time, not a runtime error. The user sees it on canvas as a dangling outcome (red highlight).

If multiple edges from same outcome port: also a validation error. One outcome → one edge.

### Hook-rejected outcomes

If a node has `OnOutcome` hook configured, and the hook returns non-zero exit:
1. The reported outcome is **rejected**.
2. Event written: `OutcomeRejectedByHook { outcome, hook_id }`.
3. Stage attempt counter increments.
4. If retries available: `StageEntered` with new attempt number.
5. If retries exhausted: stage fails with `StageFailed { reason: HookExhaustion }`.

This makes hooks a verification layer that turns soft "I think I'm done" into hard "I'm done and the contract is satisfied".

## Validation rules

A graph is valid if all the following pass:

### Structural

1. `start` node ID must exist in `nodes`.
2. Every `Edge.from.node` and `Edge.to` must reference existing nodes.
3. Every `Edge.from.outcome` must be a declared outcome on the source node.
4. Every declared outcome must have at most one outgoing edge.
5. Every node (except Terminal) must be reachable from `start` via forward edges.
6. At least one Terminal node must be reachable from every reachable node (no infinite loops without escape).

### Type-specific

7. Agent nodes must have a valid `profile` reference.
8. HumanGate nodes must have at least one `ApprovalOption`.
9. Branch nodes must have at least one `BranchArm` plus `default_outcome`.
10. Loop nodes must have a valid `iterates_over` source that produces a list.
11. Loop body must have a `start` node.
12. Subgraph inner graph must itself pass validation.

### Outcome consistency

13. If a node declares outcome `X` with `is_terminal: true`, no edge from outcome `X` is allowed.
14. Backtrack edges must form valid cycles (target node must have a path to source node via forward edges, otherwise it's not a backtrack — it's just a forward edge labeled wrong).
15. Escalate edges should point to HumanGate or Notify nodes (warning, not error).

### Validation runs

- On TOML load: full validation, refuse to load invalid graph.
- On editor save: validate, show errors inline before allowing save.
- Live in editor: incremental validation, highlighted on canvas as user edits.

## TOML serialization

Pipelines serialize to TOML. Example skeleton:

```toml
schema_version = 1

[metadata]
name = "rust-crate-tdd-medium"
description = "Adaptive flow for medium-complexity Rust crate work"
template_origin = "rust-crate-tdd@1.0"
created_at = "2026-05-01T14:32:00Z"

start = "spec_1"

# === Nodes ===

[[nodes]]
id = "spec_1"
kind = "agent"
position = { x = 100, y = 100 }
profile = "spec-author@1.0"

[[nodes.bindings]]
source = { kind = "run_artifact", name = "description.md" }
target = "{{description}}"

[[nodes.declared_outcomes]]
id = "done"
description = "Spec written and ready"
edge_kind_hint = "forward"

[[nodes.declared_outcomes]]
id = "unclear"
description = "Description has contradictions"
edge_kind_hint = "escalate"

# ... more nodes ...

# === Edges ===

[[edges]]
id = "e1"
from = { node = "spec_1", outcome = "done" }
to = "plan_1"
kind = "forward"

[[edges]]
id = "e2"
from = { node = "spec_1", outcome = "unclear" }
to = "human_clarify"
kind = "escalate"
```

Loop bodies and subgraphs are inlined as nested tables:

```toml
[[nodes]]
id = "milestone_loop"
kind = "loop"
iterates_over = { kind = "artifact", node = "roadmap_1", name = "roadmap.md", jsonpath = "$.milestones[*]" }
iteration_var_name = "milestone"
exit_condition = { kind = "all_items" }
on_iteration_failure = { kind = "retry", max = 2 }

[nodes.body]
schema_version = 1
start = "architect_in_loop"

[[nodes.body.nodes]]
id = "architect_in_loop"
kind = "agent"
profile = "architect@1.0"
# ... etc
```

## Identifiers

- **NodeId**: alphanumeric + underscore, must start with letter, max 32 chars. Examples: `spec_1`, `impl_main`, `verify_after_loop`.
- **OutcomeId**: alphanumeric + underscore, lowercase preferred, max 32 chars.
- **EdgeId**: any string, often auto-generated as `e1`, `e2`, ...
- **TemplateVar**: `{{name}}` syntax, double curly braces, alphanumeric + underscore inside.

All IDs are **stable**. They appear in event logs, are referenced from outside, and renaming requires migration.

## Acceptance criteria

The graph model is correctly implemented when:

1. A handcrafted `flow.toml` round-trips through deserialization → validation → serialization without semantic loss.
2. All seven node types can be instantiated, validated, and serialized.
3. Validation catches all 15 listed rule violations with clear error messages pointing to specific node/edge IDs.
4. The canvas editor renders a graph from `flow.toml` correctly with all positions, ports, and edges.
5. Editing on canvas (add node, connect ports, delete edge) and saving produces valid `flow.toml`.
6. A graph with nested Loop containing Subgraph containing Loop validates and serializes correctly.
7. Outcome routing test: 100 random valid graphs with synthetic outcomes → engine finds correct next node for each declared outcome.
