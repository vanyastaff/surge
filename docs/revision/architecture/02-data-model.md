# Architecture 02 · Data Model

## Overview

This document specifies concrete data types, TOML schemas for user-facing files, and SQLite schema for storage.

## Type-level overview

```
Run                                    (top-level concept)
├── RunId (UUIDv7)
├── PipelineRef (which graph)
├── Project (working directory + git context)
├── Events []                          (event log)
├── State (derived from events)
└── Worktree (filesystem layer)

Pipeline                               (graph definition)
├── Graph
│   ├── Nodes BTreeMap<NodeId, Node>
│   ├── Edges Vec<Edge>
│   └── Start NodeId
└── Metadata

Profile                                (reusable role config)
├── Role
├── Runtime
├── Sandbox
├── Tools
├── Approvals
├── Outcomes []
├── Prompt
└── InspectorUI

Event                                  (immutable record)
├── RunId
├── Seq
├── Timestamp
└── Payload (typed enum)
```

## Core types (Rust)

### IDs

Strongly-typed IDs using author's `domain-key` crate:

```rust
use domain_key::Key;

pub struct RunDomain;
pub struct NodeDomain;
pub struct EdgeDomain;
pub struct OutcomeDomain;
pub struct ProfileDomain;
pub struct ArtifactDomain;
pub struct SessionDomain;

pub type RunId = Key<RunDomain>;        // UUIDv7
pub type NodeId = Key<NodeDomain>;      // string, e.g. "impl_2"
pub type EdgeId = Key<EdgeDomain>;      // string
pub type OutcomeId = Key<OutcomeDomain>; // string
pub type ProfileRef = Key<ProfileDomain>; // "implementer@1.0"
pub type ArtifactId = Key<ArtifactDomain>; // hash-based
pub type SessionId = Key<SessionDomain>;   // ACP session ID
```

### Graph types

```rust
pub struct Graph {
    pub schema_version: u32,
    pub metadata: GraphMetadata,
    pub nodes: BTreeMap<NodeId, Node>,
    pub edges: Vec<Edge>,
    pub start: NodeId,
}

pub struct GraphMetadata {
    pub name: String,
    pub description: Option<String>,
    pub template_origin: Option<TemplateRef>,
    pub created_at: DateTime<Utc>,
    pub author: Option<String>,
}

pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub position: Position,
    pub config: NodeConfig,
    pub declared_outcomes: Vec<OutcomeDecl>,
}

pub enum NodeKind {
    Agent,
    HumanGate,
    Branch,
    Terminal,
    Notify,
    Loop,
    Subgraph,
}

pub struct Position { pub x: f32, pub y: f32 }

pub enum NodeConfig {
    Agent(AgentConfig),
    HumanGate(HumanGateConfig),
    Branch(BranchConfig),
    Terminal(TerminalConfig),
    Notify(NotifyConfig),
    Loop(LoopConfig),
    Subgraph(SubgraphConfig),
}

pub struct OutcomeDecl {
    pub id: OutcomeId,
    pub description: String,
    pub edge_kind_hint: EdgeKind,
    pub is_terminal: bool,
}

pub struct Edge {
    pub id: EdgeId,
    pub from: PortRef,
    pub to: NodeId,
    pub kind: EdgeKind,
    pub policy: EdgePolicy,
}

pub struct PortRef {
    pub node: NodeId,
    pub outcome: OutcomeId,
}

pub enum EdgeKind {
    Forward,
    Backtrack,
    Escalate,
}

pub struct EdgePolicy {
    pub max_traversals: Option<u32>,
    pub on_max_exceeded: ExceededAction,
    pub label: Option<String>,
}

pub enum ExceededAction {
    Escalate,
    Fail,
}
```

### Per-NodeKind config types

```rust
pub struct AgentConfig {
    pub profile: ProfileRef,
    pub agent: Option<String>, // named service from agents.yml
    pub prompt_overrides: Option<PromptOverride>,
    pub launch_override: Option<AgentLaunchConfig>,
    pub tool_overrides: Option<ToolOverride>,
    pub sandbox_override: Option<SandboxConfig>,
    pub approvals_override: Option<ApprovalConfig>,
    pub bindings: Vec<Binding>,
    pub rules_overrides: Option<RulesOverride>,
    pub limits: NodeLimits,
    pub hooks: Vec<Hook>,
    pub custom_fields: HashMap<String, Value>, // from inspector_ui
}

pub struct Binding {
    pub source: ArtifactSource,
    pub target: TemplateVar,
}

pub enum ArtifactSource {
    NodeOutput { node: NodeId, artifact: String },
    RunArtifact { name: String },
    GlobPattern { node: NodeId, pattern: String },
    Static { content: String },
}

pub struct TemplateVar(pub String); // "{{spec}}" stored as "spec"

pub struct NodeLimits {
    pub timeout_seconds: u32,
    pub max_retries: u32,
    pub circuit_breaker: Option<CbConfig>,
    pub max_tokens: u32,
}

pub struct CbConfig {
    pub max_failures: u32,
    pub window_seconds: u32,
    pub on_open: ExceededAction,
}

pub struct HumanGateConfig {
    pub channels: Vec<ApprovalChannel>,
    pub timeout_seconds: Option<u32>,
    pub on_timeout: TimeoutAction,
    pub summary: SummaryTemplate,
    pub options: Vec<ApprovalOption>,
    pub allow_freetext: bool,
}

// ... etc for other NodeKind variants
```

### Event types

```rust
pub struct Event {
    pub run_id: RunId,
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub payload: EventPayload,
}

pub enum EventPayload {
    // Run lifecycle
    RunStarted {
        pipeline: PipelineRef,
        project_path: PathBuf,
        initial_prompt: String,
        config: RunConfig,
    },
    RunCompleted { terminal: NodeId },
    RunFailed { error: String },
    RunAborted { reason: String },

    // Bootstrap
    BootstrapStageStarted { stage: BootstrapStage },
    BootstrapArtifactProduced { stage: BootstrapStage, artifact_id: ArtifactId },
    BootstrapApprovalRequested { stage: BootstrapStage, channel: ApprovalChannel },
    BootstrapApprovalDecided { stage: BootstrapStage, decision: BootstrapDecision, comment: Option<String> },
    BootstrapEditRequested { stage: BootstrapStage, feedback: String },

    // Pipeline
    PipelineMaterialized { graph_hash: String },

    // Stage execution
    StageEntered { node: NodeId, attempt: u32 },
    StageInputsResolved { node: NodeId, bindings: HashMap<String, ArtifactId> },
    SessionOpened {
        node: NodeId,
        session: SessionId,
        agent: String,
        launch_mode: String,
        sandbox_mode: String,
    },
    ToolCalled { session: SessionId, tool: String, args_redacted: Value },
    ToolResultReceived { session: SessionId, success: bool, result_hash: String },
    ArtifactProduced { node: NodeId, artifact_id: ArtifactId, path: PathBuf },
    OutcomeReported { node: NodeId, outcome: OutcomeId, summary: String },
    StageCompleted { node: NodeId, outcome: OutcomeId },
    StageFailed { node: NodeId, reason: String, retry_available: bool },
    SessionClosed { session: SessionId, disposition: SessionDisposition },

    // Routing
    EdgeTraversed { edge: EdgeId, from: NodeId, to: NodeId },
    LoopIterationStarted { loop_id: NodeId, item: Value, index: u32 },
    LoopIterationCompleted { loop_id: NodeId, index: u32, outcome: OutcomeId },
    LoopCompleted { loop_id: NodeId, completed_iterations: u32, final_outcome: OutcomeId },

    // Human interaction
    ApprovalRequested { gate: NodeId, channel: ApprovalChannel, payload_hash: String },
    ApprovalDecided { gate: NodeId, decision: String, channel: ApprovalChannel, comment: Option<String> },

    // Sandbox
    SandboxElevationRequested { node: NodeId, capability: String },
    SandboxElevationDecided { node: NodeId, decision: ElevationDecision, remember: bool },

    // Hooks
    HookExecuted { hook_id: String, exit_status: i32, on_failure: HookFailureMode },
    OutcomeRejectedByHook { node: NodeId, outcome: OutcomeId, hook_id: String },

    // Telemetry
    TokensConsumed { session: SessionId, prompt_tokens: u32, output_tokens: u32, cache_hits: u32 },

    // Forking
    ForkCreated { new_run: RunId, fork_at_seq: u64 },
}

pub enum BootstrapStage {
    Description,
    Roadmap,
    Flow,
}

pub enum BootstrapDecision {
    Approve,
    Edit,
    Reject,
}
```

`launch_override` is per-node by design. A graph can run different stages on different providers or execution targets while preserving deterministic routing through explicit graph edges.

## TOML schemas (user-facing files)

### `flow.toml` (pipeline definition)

```toml
schema_version = 1

[metadata]
name = "json5-parser-flow"
description = "Build JSON5 parser library"
template_origin = "rust-crate-tdd@1.0"
created_at = "2026-05-01T14:32:00Z"
author = "vanya"

start = "spec_1"

# === Nodes ===

[[nodes]]
id = "spec_1"
kind = "agent"
position = { x = 100.0, y = 100.0 }

[nodes.config]
profile = "spec-author@1.0"

[[nodes.config.bindings]]
source = { type = "run_artifact", name = "description.md" }
target = "description"

[nodes.config.limits]
timeout_seconds = 600
max_retries = 2
max_tokens = 100000

[[nodes.declared_outcomes]]
id = "done"
description = "Spec written and ready"
edge_kind_hint = "forward"
is_terminal = false

[[nodes.declared_outcomes]]
id = "unclear"
description = "Description has contradictions"
edge_kind_hint = "escalate"
is_terminal = false

# ... more nodes ...

[[nodes]]
id = "review_1"
kind = "agent"
position = { x = 700.0, y = 100.0 }

[nodes.config]
profile = "reviewer@1.0"
agent = "codex-review"                # named agent from agents.yml

# === Edges ===

[[edges]]
id = "e_spec_to_plan"
from = { node = "spec_1", outcome = "done" }
to = "plan_1"
kind = "forward"

[edges.policy]
max_traversals = null
on_max_exceeded = "fail"
label = ""

[[edges]]
id = "e_spec_to_clarify"
from = { node = "spec_1", outcome = "unclear" }
to = "human_clarify"
kind = "escalate"
```

### Profile TOML

See RFC-0005 for full schema. Stored at `~/.surge/profiles/<id>-<version>.toml`.

### Template TOML (`template.toml`)

```toml
schema_version = 1

[template]
id = "rust-crate-tdd"
version = "1.0"
display_name = "Rust Crate (TDD)"
description = "Test-driven development flow for Rust library crates"
applies_to = ["rust-crate", "rust-workspace-member"]
detected_by = ["Cargo.toml", "src/lib.rs"]

[hints]
# Examples used by Flow Generator as few-shot
[[hints.examples]]
description = "Build JSON5 parser with serde"
expected_archetype = "tdd-strict"
expected_complexity = "medium"

# The actual flow.toml is in ./pipeline.toml in the same directory
```

### Agent Compose (`agents.yml`)

`agents.yml` is the friendly, LLM-readable configuration layer. It is intentionally shaped like a small `docker-compose.yml`: named agent services, shared defaults, and role routing. Humans and LLMs should edit this file; generated `flow.toml` can then reference stable agent names instead of repeating provider-specific flags.

Locations, in precedence order:

1. `<project>/.surge/agents.yml`
2. `~/.surge/agents.yml`
3. built-in defaults

Example:

```yaml
version: 1

agents:
  claude-writer:
    provider: claude-code
    launch:
      mode: local
      profile: default
    sandbox:
      mode: workspace-write
    approvals:
      policy: on-request

  codex-review:
    provider: codex
    launch:
      mode: sandbox
      profile: strict-review
    sandbox:
      mode: read-only

  gemini-verify:
    provider: gemini
    launch:
      mode: local
    sandbox:
      mode: workspace-write

roles:
  implementer: claude-writer
  reviewer: codex-review
  verifier: gemini-verify

defaults:
  agent: claude-writer
```

Resolution:

- `nodes.config.agent = "codex-review"` selects a named agent from `agents.yml`.
- `nodes.config.launch_override` can still override one node for power users.
- Profile `[launch]` defaults are used when neither `agent` nor role routing selects a named agent.
- `surge doctor` validates that every named agent maps to an installed provider and supported launch mode.

### Run config (per-run runtime config)

Stored at `~/.surge/runs/<run_id>/config.toml`:

```toml
[run]
id = "0190a4b2-..."
project_path = "/home/user/projects/json5-parser"
created_at = "2026-05-01T14:28:00Z"
pipeline_template = "rust-crate-tdd@1.0"

[policy]
launch_default = "local"
sandbox_default = "workspace+network"
approval_default = "on-request"
auto_pr = true

[agents]
compose_file = "/home/user/projects/json5-parser/.surge/agents.yml"
default_agent = "claude-writer"

[telegram]
chat_id = 123456789
muted = false
```

## SQLite schema

Database file: `~/.surge/db/surge.sqlite`. Per-run databases also in `~/.surge/runs/<run_id>/events.sqlite`.

### Main database (registry)

```sql
-- Registry of runs (lightweight metadata; full data in per-run DBs)
CREATE TABLE runs (
    id TEXT PRIMARY KEY,                -- UUIDv7
    project_path TEXT NOT NULL,
    pipeline_template TEXT,
    status TEXT NOT NULL,               -- 'bootstrapping' | 'running' | 'completed' | 'failed' | 'aborted'
    started_at INTEGER NOT NULL,        -- Unix epoch ms
    ended_at INTEGER,
    daemon_pid INTEGER,                 -- if running, the daemon's PID
    UNIQUE(id)
);
CREATE INDEX idx_runs_status ON runs(status);
CREATE INDEX idx_runs_started ON runs(started_at);

-- Profile registry cache
CREATE TABLE profiles (
    id TEXT NOT NULL,
    version TEXT NOT NULL,
    file_path TEXT NOT NULL,
    role_metadata_json TEXT NOT NULL,   -- denormalized [role] block as JSON
    installed_at INTEGER NOT NULL,
    PRIMARY KEY(id, version)
);

-- Template registry cache
CREATE TABLE templates (
    id TEXT NOT NULL,
    version TEXT NOT NULL,
    file_path TEXT NOT NULL,
    metadata_json TEXT NOT NULL,
    PRIMARY KEY(id, version)
);

-- Trust state
CREATE TABLE trusted_files (
    path TEXT PRIMARY KEY,
    sha256 TEXT NOT NULL,
    trusted_at INTEGER NOT NULL
);

CREATE TABLE trusted_projects (
    path TEXT PRIMARY KEY,
    trusted_at INTEGER NOT NULL
);

-- User config (single row)
CREATE TABLE user_config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
```

### Per-run database (event log)

Each run has its own SQLite file: `~/.surge/runs/<run_id>/events.sqlite`. This isolates large event logs and allows individual runs to be archived/exported atomically.

```sql
-- Append-only event log
CREATE TABLE events (
    seq INTEGER PRIMARY KEY,            -- monotonic, no gaps
    timestamp INTEGER NOT NULL,         -- Unix epoch ms
    kind TEXT NOT NULL,                 -- discriminator (e.g., 'StageEntered')
    payload BLOB NOT NULL,              -- bincode-serialized EventPayload
    schema_version INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_events_kind ON events(kind);
CREATE INDEX idx_events_timestamp ON events(timestamp);

-- Materialized view: stage executions
CREATE TABLE stage_executions (
    node_id TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    started_seq INTEGER NOT NULL,
    ended_seq INTEGER,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    outcome TEXT,                       -- NULL if not completed
    cost_usd REAL DEFAULT 0,
    tokens_in INTEGER DEFAULT 0,
    tokens_out INTEGER DEFAULT 0,
    PRIMARY KEY(node_id, attempt)
);

-- Materialized view: artifacts
CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,                -- ArtifactId (hash-based)
    produced_by_node TEXT,
    produced_at_seq INTEGER NOT NULL,
    name TEXT NOT NULL,                 -- e.g., "spec.md"
    path TEXT NOT NULL,                 -- relative to runs/<run_id>/artifacts/
    size_bytes INTEGER NOT NULL,
    content_hash TEXT NOT NULL          -- sha256
);
CREATE INDEX idx_artifacts_node ON artifacts(produced_by_node);
CREATE INDEX idx_artifacts_name ON artifacts(name);

-- Materialized view: pending approvals
CREATE TABLE pending_approvals (
    seq INTEGER PRIMARY KEY,            -- the ApprovalRequested event seq
    node_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    requested_at INTEGER NOT NULL,
    payload_hash TEXT NOT NULL,
    delivered BOOLEAN DEFAULT FALSE,
    message_id INTEGER                  -- Telegram message_id if delivered
);
CREATE INDEX idx_approvals_node ON pending_approvals(node_id);

-- Materialized view: cost summary
CREATE TABLE cost_summary (
    metric TEXT PRIMARY KEY,            -- 'total_cost', 'total_tokens_in', etc.
    value REAL NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Materialized view: graph state at each significant transition
-- (Used to accelerate replay scrubber; can be regenerated from events)
CREATE TABLE graph_snapshots (
    at_seq INTEGER PRIMARY KEY,
    snapshot BLOB NOT NULL              -- bincode-serialized RunState
);
```

### Triggers

Materialized views are maintained by triggers on the `events` table:

```sql
CREATE TRIGGER on_stage_entered AFTER INSERT ON events
WHEN NEW.kind = 'StageEntered'
BEGIN
    INSERT INTO stage_executions (node_id, attempt, started_seq, started_at)
    VALUES (json_extract(NEW.payload_json, '$.node'),
            json_extract(NEW.payload_json, '$.attempt'),
            NEW.seq,
            NEW.timestamp);
END;

CREATE TRIGGER on_stage_completed AFTER INSERT ON events
WHEN NEW.kind = 'StageCompleted'
BEGIN
    UPDATE stage_executions
    SET ended_seq = NEW.seq,
        ended_at = NEW.timestamp,
        outcome = json_extract(NEW.payload_json, '$.outcome')
    WHERE node_id = json_extract(NEW.payload_json, '$.node')
      AND attempt = (SELECT MAX(attempt) FROM stage_executions WHERE node_id = json_extract(NEW.payload_json, '$.node'));
END;

-- ... more triggers for other event types ...
```

(In practice, payload deserialization happens in Rust — the `payload_json` column is a virtual computed column or maintained separately.)

### Migrations

Each migration is a numbered SQL file in `crates/storage/migrations/`:

```
0001_initial.sql
0002_add_pending_approvals.sql
0003_add_graph_snapshots.sql
```

`sqlx-cli` or custom migration runner applies them on first connect. Migrations are forward-only.

## Filesystem layout

```
~/.surge/
├── config.toml                        (user config)
├── agents.yml                         (global compose-like agent routing)
├── secrets.toml                       (mode 0600: bot tokens, etc.)
├── trust.toml
├── ui-state.toml                      (last-opened file, window positions)
├── state.toml                         (runtime state: current project, etc.)
│
├── db/
│   └── surge.sqlite                    (registry DB)
│
├── runs/
│   └── <run_id>/
│       ├── config.toml                (run config)
│       ├── events.sqlite              (event log)
│       ├── artifacts/                 (run-produced files)
│       │   ├── description.md
│       │   ├── roadmap.md
│       │   ├── flow.toml
│       │   ├── spec.md
│       │   └── ...
│       ├── worktree/                  (git worktree, symlink to actual)
│       └── .daemon                    (PID file if running)
│
├── profiles/                          (installed profiles)
│   ├── _bootstrap/
│   │   ├── description-author-1.0.toml
│   │   ├── roadmap-planner-1.0.toml
│   │   └── flow-generator-1.0.toml
│   ├── implementer-1.0.toml
│   ├── reviewer-1.0.toml
│   └── ...
│
├── templates/                         (installed templates)
│   ├── rust-crate-tdd/
│   │   ├── template.toml
│   │   └── pipeline.toml
│   ├── rust-cli-feature/
│   └── generic-tdd/
│
├── AGENTS.md                          (global rules)
├── global-deny.toml                   (protected-path warnings / provider deny hints)
└── logs/
    ├── engine.log
    ├── telegram.log
    └── editor.log
```

## Project layout (in user's project directory)

```
<user-project>/
├── ... (their code)
├── AGENTS.md                          (project rules, optional)
├── flow.toml                          (saved pipeline, optional, generated/edited)
└── .surge/
    ├── agents.yml                     (project compose-like agent routing, optional)
    ├── hooks/                         (project-local hooks)
    │   ├── check_guard.sh
    │   └── ...
    └── runs/                          (symlinks to ~/.surge/runs/<id> for runs in this project)
```

The `.surge/` directory in the project is small — agent routing, hooks, and convenience symlinks. Real run data is in `~/.surge/runs/`.

## Acceptance criteria

The data model is correctly implemented when:

1. All Rust types defined above compile and are reachable from the public API of `core` crate.
2. Round-trip: serialize a `Graph` to TOML → parse back → semantic equality (modulo whitespace).
3. Round-trip: serialize all `EventPayload` variants to bincode → deserialize → equality.
4. SQLite schema can be created from migration files on a fresh DB.
5. Triggers correctly maintain materialized views: insert 100 events of various types, query views, verify they match expected aggregates.
6. Filesystem layout: starting from empty `~/.surge/`, completing a full run produces all expected directories and files.
7. Foreign-key-like consistency: every `node_id` in events references a node in the materialized graph (caught by validation).
8. Schema versioning: an old-version event log can be upgraded to current via migration chain.
