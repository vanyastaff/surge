# M1 — `surge-core` adaptation toward vibe-flow data model

> Status: design (approved 2026-05-02)
> Scope: milestone M1 from [docs/revision/ROADMAP.md](../../revision/ROADMAP.md) — adapted for in-place evolution of Surge.
> References: [docs/revision/0003-graph-model.md](../../revision/0003-graph-model.md), [0002-execution-model.md](../../revision/0002-execution-model.md), [0005-profiles-and-roles.md](../../revision/0005-profiles-and-roles.md), [02-data-model.md](../../revision/02-data-model.md).

## 1. Goal

Add the foundational data model for the new vibe-flow architecture (graph-based pipelines, event-sourced runs, profile registry) into `surge-core` **without breaking any existing consumer**. After this milestone:

- New types `Graph`, `Node`, `Edge`, `Profile`, `RunEvent`, `RunState` are public API of `surge-core`.
- TOML round-trip for `Graph` and `Profile` works.
- Bincode round-trip for `EventPayload` works.
- Graph validation enforces 15 structural rules from [RFC-0003 §validation](../../revision/0003-graph-model.md#validation-rules).
- The fold function `apply(state, &event) -> RunState` is pure and deterministic.
- All existing types (`SurgeConfig`, `TaskState`, `Spec`, `SurgeEvent`, etc.) remain unchanged and untouched.

The vibe-flow name is an internal codename for the new architecture; the project, crate names, and public branding stay **Surge**.

## 2. Strategy

### 2.1 Pure addition, flat layout

Decisions locked in during brainstorming:

- **Pure addition.** No `#[deprecated]` attributes, no module renames. Both old and new types coexist in `surge-core`. Migration of consumers (orchestrator, CLI, persistence) happens in later milestones, file by file.
- **Flat module structure.** All new modules live directly under `surge-core/src/` next to existing files. No `vibe::` / `flow::` sub-namespace. Rationale: simpler imports, IDE navigation by filename, consistent with the existing flat layout.
- **One spec, phased implementation.** This document is the single design contract for all of M1; the implementation plan (produced via `superpowers:writing-plans` after this spec is approved) sequences the work into reviewable chunks.

### 2.2 ID strategy — split by semantics

| Class | Examples | Implementation |
|---|---|---|
| Stable user-typed strings | `NodeKey`, `EdgeKey`, `OutcomeKey`, `ProfileKey`, `TemplateKey` | [`domain-key`](https://docs.rs/domain-key) crate (new workspace dep). One marker domain per key. |
| Auto-generated runtime IDs | `RunId`, `SessionId` | Existing `define_id!` macro in `id.rs` (ULID + textual prefix), consistent with `SpecId`/`TaskId`/`SubtaskId`. |
| Content-addressed | `ContentHash` | Dedicated `ContentHash([u8; 32])` newtype with `sha256:hex` display. |

Rationale: `domain-key` stores `SmartString` internally — natural fit for human-typed identifiers in `flow.toml` (`"impl_2"`, `"done"`, `"implementer@1.0"`), but wasteful for binary 128-bit ULIDs. The split keeps each tool used where it shines.

`SpecId` / `TaskId` / `SubtaskId` stay as they are — they belong to the legacy task-FSM model; they will not be replaced by the new keys, they will simply become unused once the relevant subsystems migrate (later milestones).

## 3. Module layout after M1

All files at top level of `surge-core/src/`. Legacy modules untouched.

```
surge-core/src/
├── error.rs                    (extended with new error variants)
├── lib.rs                      (extended re-exports)
│
│  ── legacy (no changes in M1) ──
├── config.rs                   SurgeConfig, AgentConfig (legacy)
├── event.rs                    SurgeEvent, VersionedEvent (legacy)
├── id.rs                       SpecId, TaskId, SubtaskId  + RunId, SessionId (extension)
├── roadmap.rs
├── spec.rs                     legacy Spec, Subtask
├── state.rs                    TaskState
│
│  ── new ──
├── graph.rs                    Graph, Subgraph, GraphMetadata, schema_version constants
├── node.rs                     Node, NodeKind, NodeConfig, Position, OutcomeDecl
├── edge.rs                     Edge, EdgeKind, EdgePolicy, PortRef, ExceededAction
├── agent_config.rs             AgentConfig, Binding, ArtifactSource, NodeLimits, CbConfig, PromptOverride, ToolOverride, RulesOverride, TemplateVar
├── human_gate_config.rs        HumanGateConfig, ApprovalOption, OptionStyle, SummaryTemplate, TimeoutAction
├── branch_config.rs            BranchConfig, BranchArm, Predicate, CompareOp
├── terminal_config.rs          TerminalConfig, TerminalKind
├── notify_config.rs            NotifyConfig, NotifyTemplate, NotifyChannel, NotifySeverity, NotifyFailureAction
├── loop_config.rs              LoopConfig (body: SubgraphKey), IterableSource, ExitCondition, FailurePolicy, ParallelismMode
├── subgraph_config.rs          SubgraphConfig (inner: SubgraphKey), SubgraphInput, SubgraphOutput
├── sandbox.rs                  SandboxConfig, SandboxMode
├── approvals.rs                ApprovalConfig (elevation_channels), ApprovalPolicy, ApprovalChannel
├── hooks.rs                    Hook, HookTrigger, HookFailureMode, HookInheritance, MatcherSpec, MatchContext
├── validation.rs               validate(&Graph) -> Result<(), Vec<ValidationError>>, ValidationError
├── profile.rs                  Profile, Role, RuntimeCfg, ToolsCfg, PromptTemplate, InspectorUiField, ProfileBindings, ExpectedBinding
├── run_event.rs                RunEvent, EventPayload (~30 variants), VersionedEventPayload, BootstrapStage, BootstrapDecision, BootstrapSubstate, ApprovalDecision, ElevationDecision, SessionDisposition, HookFailureMode (re-export)
├── run_state.rs                RunState (Pipeline holds Arc<Graph>), RunMemory, Cursor, fold(), apply_event()
├── keys.rs                     NodeKey, EdgeKey, OutcomeKey, SubgraphKey, ProfileKey, TemplateKey + KeyDomain markers
└── content_hash.rs             ContentHash([u8; 32])
```

**File count**: 27 files total in `surge-core/src/` after M1: 2 shared (`error.rs`, `lib.rs`) + 6 legacy (untouched) + 19 new. Largest file expected: `run_event.rs` (~600 lines for ~30 event variants), `agent_config.rs` (~300), `profile.rs` (~250). Everything else < 200 lines.

## 4. Type specifications

Field-by-field specs follow. Each section corresponds to one file. All types derive `Debug`, `Clone`, `Serialize`, `Deserialize` unless noted. Defaults marked via `#[serde(default)]` with `Default` impls or `#[serde(default = "fn")]` factories.

### 4.1 `keys.rs`

```rust
use domain_key::{define_domain, key_type, KeyDomain};

define_domain!(NodeDomain, "node", 32);
define_domain!(EdgeDomain, "edge", 32);
define_domain!(OutcomeDomain, "outcome", 32);
define_domain!(SubgraphDomain, "subgraph", 32);
define_domain!(ProfileDomain, "profile", 64);    // includes "@version" suffix
define_domain!(TemplateDomain, "template", 64);

key_type!(NodeKey, NodeDomain);
key_type!(EdgeKey, EdgeDomain);
key_type!(OutcomeKey, OutcomeDomain);
key_type!(SubgraphKey, SubgraphDomain);
key_type!(ProfileKey, ProfileDomain);
key_type!(TemplateKey, TemplateDomain);
```

Validation rules for keys:
- `NodeKey`/`EdgeKey`/`OutcomeKey`/`SubgraphKey`: alphanumeric + underscore, must start with letter, max 32 chars.
- `ProfileKey`/`TemplateKey`: alphanumeric + `-` + `_` + `.` + `@`, max 64 chars (e.g. `"implementer@1.0"`).

`domain-key`'s `MAX_LENGTH` constant covers length enforcement. Character-set validation is implemented as a thin wrapper module:

```rust
pub fn parse_node_key(s: &str) -> Result<NodeKey, KeyParseError> {
    validate_charset_strict(s)?;     // alphanumeric + underscore, leading letter
    NodeKey::try_from(s).map_err(KeyParseError::from)
}

pub fn parse_profile_key(s: &str) -> Result<ProfileKey, KeyParseError> {
    validate_charset_extended(s)?;   // also -, ., @
    ProfileKey::try_from(s).map_err(KeyParseError::from)
}
```

Custom serde `Deserialize` impls on each key route through these parsers so invalid keys in `flow.toml` produce structured errors at parse time, not at validation time.

### 4.2 `content_hash.rs`

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    pub fn from_bytes(bytes: [u8; 32]) -> Self { Self(bytes) }
    pub fn compute(content: &[u8]) -> Self { /* sha2::Sha256 */ }
    pub fn as_bytes(&self) -> &[u8; 32] { &self.0 }
    pub fn to_hex(&self) -> String { hex::encode(self.0) }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sha256:{}", self.to_hex())
    }
}

impl std::fmt::Debug for ContentHash { /* same as Display */ }

impl std::str::FromStr for ContentHash {
    type Err = ContentHashParseError;
    /// Parses `"sha256:<64 hex chars>"` (prefix optional).
    fn from_str(s: &str) -> Result<Self, Self::Err> { ... }
}

// Custom serde — TOML-friendly: serializes as the `"sha256:..."` string,
// not as a byte array.
impl Serialize for ContentHash { /* writes Display */ }
impl<'de> Deserialize<'de> for ContentHash { /* reads via FromStr */ }
```

`sha2` and `hex` crates added as workspace deps.

### 4.3 `id.rs` (extension)

```rust
// existing macro, existing legacy IDs untouched
define_id!(SpecId, "spec");
define_id!(TaskId, "task");
define_id!(SubtaskId, "sub");

// new — added in M1
define_id!(RunId, "run");
define_id!(SessionId, "session");
```

`RunId` and `SessionId` are ULID-based via the existing macro. The macro already provides `Display` (with prefix) and `FromStr` (accepts both prefixed and bare).

### 4.4 `graph.rs`

```rust
pub const SCHEMA_VERSION: u32 = 1;

/// Top-level pipeline graph. One per `flow.toml`.
pub struct Graph {
    pub schema_version: u32,
    pub metadata: GraphMetadata,
    /// Top-level (root) graph contents.
    pub start: NodeKey,
    pub nodes: BTreeMap<NodeKey, Node>,
    pub edges: Vec<Edge>,
    /// Library of named subgraphs. `Loop.body` and `Subgraph.inner` reference
    /// entries here by `SubgraphKey`. Always lives at the root — subgraphs
    /// cannot themselves contain a `subgraphs` field.
    #[serde(default)]
    pub subgraphs: BTreeMap<SubgraphKey, Subgraph>,
}

/// A named, reusable inner graph. Lighter than `Graph` — no metadata, no nested
/// subgraphs library (those would all live at the root `Graph` level).
pub struct Subgraph {
    pub start: NodeKey,
    pub nodes: BTreeMap<NodeKey, Node>,
    pub edges: Vec<Edge>,
}

pub struct GraphMetadata {
    pub name: String,
    pub description: Option<String>,
    pub template_origin: Option<TemplateKey>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub author: Option<String>,
}
```

Notes:
- `nodes: BTreeMap<NodeKey, Node>` — O(log n) lookup, deterministic serialization order (alphabetical by key). Edge resolution looks up nodes by key on every routing decision; this is the hot path.
- `edges: Vec<Edge>` — iterated more than indexed; stays a vec.
- `subgraphs: BTreeMap<SubgraphKey, Subgraph>` — **flat** library of named subgraphs at the root. `LoopConfig.body` and `SubgraphConfig.inner` carry a `SubgraphKey` that resolves into this map (decision: §4.4.1 below).
- `chrono` is a new workspace dep.

#### 4.4.1 Why subgraphs are flat (not inline `Box<Graph>`)

The earlier draft modeled `LoopConfig.body: Box<Graph>` and `SubgraphConfig.inner: Box<Graph>` — full inline recursion. This fails in TOML for non-trivial nesting:

- 3-level nested loop produces `[nodes.task_loop.config.body.nodes.inner_loop.config.body.nodes.deep_impl]` — readability collapses.
- Editing the body of an inner subgraph in a text editor requires expanding nested tables that grow O(depth).
- Sharing a subgraph across multiple Loop nodes (e.g., one common task body invoked in two outer Milestone Loops) has no representation.

The flat-library design solves this:

```toml
# Root graph
schema_version = 1
start = "spec_1"

[[nodes]]
id = "milestone_loop"

[nodes.config]
kind = "loop"
body = "task_loop_body"          # ← SubgraphKey ref into root.subgraphs
iterates_over = { ... }
# ...

# Named subgraph at root
[subgraphs.task_loop_body]
start = "implement_inner"

[[subgraphs.task_loop_body.nodes]]
id = "implement_inner"
# ...

[[subgraphs.task_loop_body.edges]]
# ...
```

Trade-off: the root `Graph` is no longer fully self-contained-by-position (you can't grep one node's config and see its full execution context inline), but it is flat in TOML, shareable across nodes, and the editor's "open body" view becomes a simple tab switch within the same file rather than navigating nested tables.

A 3-level nesting fixture (`tests/fixtures/graphs/nested-3-levels.toml`) is part of M1 acceptance to validate the format works in practice.

#### 4.4.2 `NodeKey` scope: globally unique across `Graph` + all `Subgraph`s

`NodeKey` is a single namespace spanning the entire graph file. The same `NodeKey` value may not appear in `Graph::nodes` and any `Subgraph::nodes`, nor in two different subgraphs. Validation enforces this (rule 17 in §4.17).

Why global, not per-subgraph:

- **Event log unambiguity**: `EventPayload::StageEntered { node: NodeKey }` carries no enclosing-subgraph context, and the engine doesn't have to pair every event with "which subgraph is active right now". A `NodeKey` in an event is a complete address.
- **Routing simplicity**: edges within a subgraph reference target nodes by `NodeKey` only (no qualified path). Folding/replay treats keys uniformly without a context stack.
- **TOML tooling**: a project-wide grep for `id = "implementer_inner"` always returns at most one definition. With per-subgraph namespacing it could match dozens of unrelated copies, all of which serve different purposes.

Cost: users must invent unique node IDs across the whole pipeline. For declarative TOML this is normal — the same constraint exists in HCL, Terraform, GitHub Actions workflow files, etc. Convention encouraged in tooling: prefix or suffix the subgraph's own role (e.g., `task_loop_implement`, `milestone_review`, `final_pr_compose`) — the editor's lint can suggest fixes when a collision is detected.

`SubgraphKey` is a separate namespace from `NodeKey` (different `domain-key` domain), so there's no conflict between e.g. a node named `"foo"` and a subgraph named `"foo"`. They live in disjoint maps.

### 4.5 `node.rs`

```rust
pub struct Node {
    pub id: NodeKey,
    pub position: Position,
    pub declared_outcomes: Vec<OutcomeDecl>,
    pub config: NodeConfig,
}

impl Node {
    pub fn kind(&self) -> NodeKind { self.config.kind() }
}

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Agent, HumanGate, Branch, Terminal, Notify, Loop, Subgraph,
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeConfig {
    Agent(AgentConfig),
    HumanGate(HumanGateConfig),
    Branch(BranchConfig),
    Terminal(TerminalConfig),
    Notify(NotifyConfig),
    Loop(LoopConfig),
    Subgraph(SubgraphConfig),
}

impl NodeConfig {
    pub fn kind(&self) -> NodeKind { /* exhaustive match */ }
}

#[derive(Copy, PartialEq)]
pub struct Position { pub x: f32, pub y: f32 }

pub struct OutcomeDecl {
    pub id: OutcomeKey,
    pub description: String,
    pub edge_kind_hint: EdgeKind,
    #[serde(default)]
    pub is_terminal: bool,
}
```

**Deviation from RFC-0003**: `Node` does **not** carry a separate `kind: NodeKind` field; the kind is encoded in the `NodeConfig` variant tag (serde `#[serde(tag = "kind")]`). The `Node::kind()` accessor exposes it. This eliminates the spec's redundancy where `Node.kind` and `Node.config`'s tag both had to be kept in sync.

`NodeKind` is a closed enum without `#[non_exhaustive]` — extending it requires editing core (per RFC design intent).

### 4.6 `edge.rs`

```rust
pub struct Edge {
    pub id: EdgeKey,
    pub from: PortRef,
    pub to: NodeKey,
    pub kind: EdgeKind,
    #[serde(default)]
    pub policy: EdgePolicy,
}

#[derive(PartialEq, Eq, Hash)]
pub struct PortRef {
    pub node: NodeKey,
    pub outcome: OutcomeKey,
}

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind { Forward, Backtrack, Escalate }

#[derive(Default)]
pub struct EdgePolicy {
    pub max_traversals: Option<u32>,
    #[serde(default)]
    pub on_max_exceeded: ExceededAction,
    pub label: Option<String>,
}

#[derive(Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExceededAction {
    #[default]
    Escalate,
    Fail,
}
```

### 4.7 `sandbox.rs`

```rust
pub struct SandboxConfig {
    pub mode: SandboxMode,
    #[serde(default)]
    pub writable_roots: Vec<PathBuf>,
    #[serde(default)]
    pub network_allowlist: Vec<String>,
    #[serde(default)]
    pub shell_allowlist: Vec<String>,
    #[serde(default)]
    pub protected_paths: Vec<String>,
}

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    WorkspaceNetwork,    // serializes as "workspace-network"
    FullAccess,
    Custom,
}
```

### 4.8 `approvals.rs`

```rust
pub struct ApprovalConfig {
    pub policy: ApprovalPolicy,
    #[serde(default)] pub sandbox_approval: bool,
    #[serde(default)] pub mcp_elicitations: bool,
    #[serde(default)] pub request_permissions: bool,
    #[serde(default)] pub skill_approval: bool,
    #[serde(default)] pub elevation: bool,
    /// Channels where sandbox-elevation requests and other agent-stage
    /// approval prompts get delivered. Distinct from `HumanGateConfig::delivery_channels`
    /// (which is for explicit gate prompts). Field name disambiguates intent.
    #[serde(default)] pub elevation_channels: Vec<ApprovalChannel>,
}

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalPolicy { Untrusted, OnRequest, Never }

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalChannel {
    Telegram { chat_id_ref: String },
    Desktop  { duration: ApprovalDuration },
    Email    { to_ref: String },
    Webhook  { url: String },
}

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDuration { Persistent, Transient }
```

### 4.9 `hooks.rs`

```rust
pub struct Hook {
    pub id: String,
    pub trigger: HookTrigger,
    /// Structured match expression. Empty matcher (default) = always matches.
    /// String-DSL is **not** supported — we want type-checked configurations
    /// at parse time, not runtime expression-eval surprises.
    #[serde(default)]
    pub matcher: MatcherSpec,
    pub command: String,
    #[serde(default)]
    pub on_failure: HookFailureMode,
    pub timeout_seconds: Option<u32>,
    #[serde(default)]
    pub inherit: HookInheritance,
}

/// Structured matcher. Each field is an additional `AND` constraint;
/// an empty `MatcherSpec` (all fields `None`) matches every event.
/// New fields are added as needed when triggers gain new context.
#[derive(Default)]
pub struct MatcherSpec {
    /// Match by tool name (for `pre_tool_use` / `post_tool_use`).
    pub tool: Option<String>,
    /// Match by outcome (for `on_outcome`).
    pub outcome: Option<OutcomeKey>,
    /// Match by node key.
    pub node: Option<NodeKey>,
    /// Substring match against tool args (best-effort; for `pre/post_tool_use`).
    pub tool_arg_contains: Option<String>,
    /// Match by file path glob (for `on_outcome` / `pre_tool_use` with file ops).
    pub file_glob: Option<String>,
}

impl MatcherSpec {
    /// Returns `true` if this matcher is empty (matches everything).
    pub fn is_unconditional(&self) -> bool;
    /// Evaluates the matcher against a `MatchContext`. Pure function — engine
    /// builds the `MatchContext` from the current event before calling.
    pub fn matches(&self, ctx: &MatchContext) -> bool;
}

pub struct MatchContext<'a> {
    pub trigger: HookTrigger,
    pub tool: Option<&'a str>,
    pub tool_args_text: Option<&'a str>,
    pub outcome: Option<&'a OutcomeKey>,
    pub node: Option<&'a NodeKey>,
    pub file_path: Option<&'a Path>,
}

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookTrigger { PreToolUse, PostToolUse, OnOutcome, OnError }

#[derive(Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookFailureMode {
    Reject,
    #[default] Warn,
    Ignore,
}

#[derive(Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookInheritance {
    #[default] Extend,
    Replace,
    Disable,
}
```

**Rationale**: the original RFC said `matcher: String` with "JS-like predicate" — that's a footgun. By M5 we'd need a parser, an evaluator, and tests that catch every weird edge case in user-written expressions. A typed `MatcherSpec` covers the documented use cases (`tool == "edit_file"`, `outcome == "done"`, file-path globs) at parse time. Extending it later is a backward-compatible additive change to the struct. If a real need emerges for free-form expressions (unlikely from the RFC's examples), we add a `custom: Option<String>` field with a documented mini-DSL — but only with a real driver, not speculation.

### 4.10 `agent_config.rs`

```rust
pub struct AgentConfig {
    pub profile: ProfileKey,
    #[serde(default)] pub prompt_overrides: Option<PromptOverride>,
    #[serde(default)] pub tool_overrides: Option<ToolOverride>,
    #[serde(default)] pub sandbox_override: Option<SandboxConfig>,
    #[serde(default)] pub approvals_override: Option<ApprovalConfig>,
    #[serde(default)] pub bindings: Vec<Binding>,
    #[serde(default)] pub rules_overrides: Option<RulesOverride>,
    #[serde(default)] pub limits: NodeLimits,
    #[serde(default)] pub hooks: Vec<Hook>,
    #[serde(default)] pub custom_fields: BTreeMap<String, toml::Value>,
}

pub struct Binding {
    pub source: ArtifactSource,
    pub target: TemplateVar,
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArtifactSource {
    NodeOutput   { node: NodeKey, artifact: String },
    RunArtifact  { name: String },
    GlobPattern  { node: NodeKey, pattern: String },
    Static       { content: String },
}

#[derive(PartialEq, Eq, Hash)]
pub struct TemplateVar(pub String);  // stores name without curly braces

pub struct PromptOverride {
    pub system: Option<String>,
    pub append_system: Option<String>,
}

#[derive(Default)]
pub struct ToolOverride {
    #[serde(default)] pub mcp_add: Vec<String>,
    #[serde(default)] pub mcp_remove: Vec<String>,
    #[serde(default)] pub skills_add: Vec<String>,
    #[serde(default)] pub skills_remove: Vec<String>,
    #[serde(default)] pub shell_allowlist_add: Vec<String>,
}

#[derive(Default)]
pub struct RulesOverride {
    #[serde(default)] pub disable_inherited: bool,
    #[serde(default)] pub additional_rules: Vec<String>,
}

pub struct NodeLimits {
    #[serde(default = "default_timeout")]      pub timeout_seconds: u32,    // 900
    #[serde(default = "default_max_retries")]  pub max_retries: u32,        // 3
    #[serde(default)]                          pub circuit_breaker: Option<CbConfig>,
    #[serde(default = "default_max_tokens")]   pub max_tokens: u32,         // 200_000
}

pub struct CbConfig {
    pub max_failures: u32,
    pub window_seconds: u32,
    pub on_open: crate::edge::ExceededAction,
}
```

`custom_fields` uses `BTreeMap<String, toml::Value>` (deterministic order, TOML-native value type) instead of `HashMap`.

### 4.11 `human_gate_config.rs`

```rust
pub struct HumanGateConfig {
    /// Channels where the gate's approval card is sent, in priority order.
    /// Distinct from `ApprovalConfig::elevation_channels` (which is for sandbox
    /// elevation prompts on agent stages). Field name disambiguates intent.
    pub delivery_channels: Vec<ApprovalChannel>,
    pub timeout_seconds: Option<u32>,
    #[serde(default)] pub on_timeout: TimeoutAction,
    pub summary: SummaryTemplate,
    pub options: Vec<ApprovalOption>,
    #[serde(default)] pub allow_freetext: bool,
}

#[derive(Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutAction {
    #[default] Reject,
    Escalate,
    Continue,
}

pub struct ApprovalOption {
    pub outcome: OutcomeKey,
    pub label: String,
    #[serde(default)] pub style: OptionStyle,
}

#[derive(Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OptionStyle {
    Primary, Danger, Warn,
    #[default] Normal,
}

pub struct SummaryTemplate {
    pub title: String,
    pub body: String,
    #[serde(default)] pub show_artifacts: Vec<ArtifactSource>,
}
```

### 4.12 `branch_config.rs`

```rust
pub struct BranchConfig {
    pub predicates: Vec<BranchArm>,
    pub default_outcome: OutcomeKey,
}

pub struct BranchArm {
    pub condition: Predicate,
    pub outcome: OutcomeKey,
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum Predicate {
    FileExists      { path: String },
    ArtifactSize    { artifact: String, op: CompareOp, value: u64 },
    OutcomeMatches  { node: NodeKey, outcome: OutcomeKey },
    EnvVar          { name: String, op: CompareOp, value: String },
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Not(Box<Predicate>),
}

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp { Eq, Ne, Lt, Lte, Gt, Gte }
```

### 4.13 `terminal_config.rs`

```rust
pub struct TerminalConfig {
    pub kind: TerminalKind,
    pub message: Option<String>,
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalKind {
    Success,
    Failure { exit_code: i32 },
    Aborted,
}
```

### 4.14 `notify_config.rs`

```rust
pub struct NotifyConfig {
    pub channel: NotifyChannel,
    pub template: NotifyTemplate,
    #[serde(default)] pub on_failure: NotifyFailureAction,
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum NotifyChannel {
    Telegram { chat_id_ref: String },
    Slack    { channel_ref: String },
    Email    { to_ref: String },
    Desktop,
    Webhook  { url: String },
}

pub struct NotifyTemplate {
    pub severity: NotifySeverity,
    pub title: String,
    pub body: String,
    #[serde(default)] pub artifacts: Vec<ArtifactSource>,
}

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotifySeverity { Info, Warn, Error, Success }

#[derive(Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotifyFailureAction {
    #[default] Continue,
    Fail,
}
```

### 4.15 `loop_config.rs`

```rust
pub struct LoopConfig {
    pub iterates_over: IterableSource,
    /// Subgraph to execute per iteration. References `Graph::subgraphs[body]`.
    /// Validation enforces existence (rule 11b in §4.17).
    pub body: SubgraphKey,
    pub iteration_var_name: String,
    pub exit_condition: ExitCondition,
    #[serde(default)] pub on_iteration_failure: FailurePolicy,
    #[serde(default)] pub parallelism: ParallelismMode,
    #[serde(default)] pub gate_after_each: bool,
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum IterableSource {
    Artifact { node: NodeKey, name: String, jsonpath: String },
    Static(Vec<toml::Value>),
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExitCondition {
    AllItems,
    UntilOutcome   { from_node: NodeKey, outcome: OutcomeKey },
    MaxIterations  { n: u32 },
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum FailurePolicy {
    Abort,
    Skip,
    Retry { max: u32 },
    Replan,
}

impl Default for FailurePolicy { fn default() -> Self { Self::Abort } }

#[derive(Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParallelismMode {
    #[default] Sequential,
    // Parallel { max_concurrent: u32 } — reserved for v2
}
```

### 4.16 `subgraph_config.rs`

```rust
pub struct SubgraphConfig {
    /// Inner subgraph to execute. References `Graph::subgraphs[inner]`.
    /// Validation enforces existence.
    pub inner: SubgraphKey,
    pub inputs: Vec<SubgraphInput>,
    pub outputs: Vec<SubgraphOutput>,
}

pub struct SubgraphInput {
    pub outer_binding: Binding,
    pub inner_var: TemplateVar,
}

pub struct SubgraphOutput {
    pub inner_artifact: ArtifactSource,
    pub outer_outcome: OutcomeKey,
}
```

### 4.17 `validation.rs`

Public API:

```rust
pub fn validate(graph: &Graph) -> Result<(), Vec<ValidationError>>;

pub struct ValidationError {
    pub kind: ValidationErrorKind,
    pub location: ErrorLocation,
    pub message: String,
}

pub enum ErrorLocation {
    Graph,
    Node      { id: NodeKey },
    Edge      { id: EdgeKey },
    Outcome   { node: NodeKey, outcome: OutcomeKey },
    Subgraph  { path: Vec<NodeKey> },        // path of loop/subgraph node IDs leading down
}

pub enum ValidationErrorKind {
    StartNodeMissing,
    EdgeFromUnknownNode,
    EdgeToUnknownNode,
    EdgeFromUndeclaredOutcome,
    DuplicateEdgeFromSamePort,
    OutcomeWithNoEdge,
    UnreachableNode,
    NoTerminalReachable,
    InvalidProfileRef,
    HumanGateWithoutOptions,
    BranchWithoutArms,
    LoopIterableInvalid,
    LoopBodyMissingStart,
    SubgraphInvalid,
    TerminalOutcomeHasEdge,
    BacktrackTargetUnreachable,
    EscalateTargetNotHumanOrNotify,    // warning, not error
    SchemaVersionMismatch,
    KeyFormatViolation { key: String },
    SubgraphRefMissing { subgraph: SubgraphKey },
    SubgraphReferenceCycle { cycle: Vec<SubgraphKey> },
    NodeKeyCollision { key: NodeKey, locations: Vec<NodeKeyOrigin> },
    OrphanSubgraph { key: SubgraphKey },    // warning, not error
}

/// Where a colliding `NodeKey` was found.
pub enum NodeKeyOrigin {
    Root,
    Subgraph(SubgraphKey),
}

/// Severity classification — caller decides how to surface.
pub enum Severity { Error, Warning }

impl ValidationErrorKind {
    pub fn severity(&self) -> Severity {
        match self {
            Self::EscalateTargetNotHumanOrNotify | Self::OrphanSubgraph { .. } => Severity::Warning,
            _ => Severity::Error,
        }
    }
}
```

**15 rules** (mapping to rule numbers in [RFC-0003 §validation](../../revision/0003-graph-model.md#validation-rules)):

1. `start` node ID exists in `nodes`.
2. Every `Edge.from.node` references an existing node.
3. Every `Edge.to` references an existing node.
4. Every `Edge.from.outcome` is a declared outcome on the source node.
5. Every declared outcome has at most one outgoing edge.
6. Every node (except Terminal) is reachable from `start` via forward edges.
7. At least one Terminal node is reachable from every reachable node (no infinite-loop traps).
8. `Agent` nodes have a valid `profile` field (well-formed `ProfileKey`).
9. `HumanGate` nodes have at least one `ApprovalOption`.
10. `Branch` nodes have at least one `BranchArm` plus `default_outcome`.
11. `Loop` nodes have a valid `iterates_over` source.
11b. `Loop.body` and `Subgraph.inner` reference an existing `SubgraphKey` in `Graph::subgraphs`.
12. Every subgraph has a `start` node that exists in its own `nodes`.
13. Each `Subgraph` in `Graph::subgraphs` itself passes the same structural rules (rules 1–6, 8–11, 14–15 applied with the subgraph as the local graph; rules concerning `subgraphs` field are skipped since `Subgraph` has none).
14. If outcome `is_terminal: true`, no edge from that outcome (terminal outcomes have no successors).
15. `Backtrack` edges form valid cycles (target reachable from source via forward edges).
16. Subgraph reference graph is acyclic: no `Subgraph` A's `Loop`/`Subgraph` nodes reach a chain that points back into A. Walk via Tarjan or simple DFS with seen-set; report the cycle as a `Vec<SubgraphKey>`.
17. `NodeKey` global uniqueness: every node ID in `Graph::nodes` ∪ `Graph::subgraphs[*].nodes` must be unique across the whole file (per §4.4.2).

Plus warnings (non-error):
- Rule W1: `Escalate` edges should target `HumanGate` or `Notify` nodes.
- Rule W2: orphan subgraphs — entries in `Graph::subgraphs` that no `Loop.body` or `Subgraph.inner` references. Defined-but-unused subgraphs are likely user mistakes (typo in reference, or leftover after deletion). Report as `ValidationErrorKind::OrphanSubgraph { key }` with severity `Warning` so editor highlights but doesn't block save.

Plus warning (rule 16, non-error):
- `Escalate` edges should target `HumanGate` or `Notify` nodes.

**Strategy**: validation is **non-fail-fast** — all errors collected into `Vec<ValidationError>`, so the editor can highlight every problem at once. Only abort early on rules that prevent further analysis (e.g., `start` missing makes reachability undefined — collect that and skip reachability checks). Warnings are returned in the same vector with `Severity::Warning`; callers (CLI vs editor) decide whether to fail-on-warning or just display them.

**Subgraph traversal**: validation iterates flat — each `Subgraph` in `Graph::subgraphs` is checked once for structural rules, then a separate cycle detector walks the subgraph reference graph (`Loop.body` / `Subgraph.inner` → next subgraph), reporting `SubgraphReferenceCycle` if a cycle exists. `ErrorLocation::Subgraph.path` lists the chain of subgraph references that led to an error inside a deep subgraph. No stack-recursive walks; depth is bounded by the (acyclic) subgraph graph.

### 4.18 `profile.rs`

```rust
pub struct Profile {
    pub schema_version: u32,
    pub role: Role,
    pub runtime: RuntimeCfg,
    #[serde(default)] pub sandbox: SandboxConfig,
    #[serde(default)] pub tools: ToolsCfg,
    #[serde(default)] pub approvals: ApprovalConfig,
    pub outcomes: Vec<ProfileOutcome>,
    #[serde(default)] pub bindings: ProfileBindings,
    #[serde(default)] pub hooks: ProfileHooks,
    pub prompt: PromptTemplate,
    #[serde(default)] pub inspector_ui: InspectorUi,
}

pub struct Role {
    pub id: ProfileKey,                         // "implementer"
    pub version: semver::Version,
    pub display_name: String,
    pub icon: Option<String>,
    pub category: RoleCategory,
    pub description: String,
    pub when_to_use: String,
    /// If set, this profile inherits from another (e.g. "generic-implementer@1.0").
    /// Resolution & merging happens at load time in a later milestone (engine).
    pub extends: Option<ProfileKey>,
}

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoleCategory {
    Agents,
    Gates,
    Flow,
    Io,
    #[serde(rename = "_bootstrap")]
    Bootstrap,
}

pub struct RuntimeCfg {
    pub recommended_model: String,
    #[serde(default = "default_temperature")] pub default_temperature: f32,
    #[serde(default = "default_max_tokens_profile")] pub default_max_tokens: u32,
    #[serde(default)] pub load_rules_lazily: Option<bool>,
}

#[derive(Default)]
pub struct ToolsCfg {
    #[serde(default)] pub default_mcp: Vec<String>,
    #[serde(default)] pub default_skills: Vec<String>,
    #[serde(default)] pub default_shell_allowlist: Vec<String>,
}

pub struct ProfileOutcome {
    pub id: OutcomeKey,
    pub description: String,
    pub edge_kind_hint: EdgeKind,
    #[serde(default)] pub required_artifacts: Vec<String>,    // glob patterns
}

#[derive(Default)]
pub struct ProfileBindings {
    #[serde(default)] pub expected: Vec<ExpectedBinding>,
}

pub struct ExpectedBinding {
    pub name: String,
    pub source: ExpectedBindingSource,
    #[serde(default)] pub optional: bool,
}

#[serde(tag = "source", rename_all = "snake_case")]
pub enum ExpectedBindingSource {
    NodeOutput { from_role: ProfileKey },
    RunArtifact,
    Any,
}

#[derive(Default)]
pub struct ProfileHooks {
    #[serde(default)] pub entries: Vec<Hook>,
}

pub struct PromptTemplate {
    pub system: String,                          // Handlebars-like syntax
}

#[derive(Default)]
pub struct InspectorUi {
    #[serde(default)] pub fields: Vec<InspectorUiField>,
}

pub struct InspectorUiField {
    pub id: String,
    pub label: String,
    pub kind: InspectorFieldKind,
    pub default: Option<toml::Value>,
    pub help: Option<String>,
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InspectorFieldKind {
    Number  { min: Option<f64>, max: Option<f64> },
    Toggle,
    Select  { options: Vec<String> },
    Text    { multiline: bool },
}
```

`semver` is added as a workspace dep for `Role.version`.

`Profile::extends` is parsed but **not resolved** in M1 — actual inheritance merging is engine logic, will land when the engine crate adapts in a later milestone. M1 only ensures TOML round-trip preserves the field.

### 4.19 `run_event.rs`

The new event log entry. Designed **separately from legacy `SurgeEvent`** — they have different shapes, lifetimes, and semantics. Both coexist; consumers pick which they need.

```rust
pub struct RunEvent {
    pub run_id: RunId,
    pub seq: u64,                                         // monotonic per-run, starts at 1
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub payload: EventPayload,
}

pub struct VersionedEventPayload {
    pub schema_version: u32,
    pub payload: EventPayload,
}
```

`EventPayload` variants (organized by category, ~30 total):

```rust
pub enum EventPayload {
    // ── Run lifecycle ──
    RunStarted   { pipeline_template: Option<TemplateKey>, project_path: PathBuf,
                   initial_prompt: String, config: RunConfig },
    RunCompleted { terminal_node: NodeKey },
    RunFailed    { error: String },
    RunAborted   { reason: String },

    // ── Bootstrap ──
    BootstrapStageStarted    { stage: BootstrapStage },
    BootstrapArtifactProduced{ stage: BootstrapStage, artifact: ContentHash, name: String },
    BootstrapApprovalRequested { stage: BootstrapStage, channel: ApprovalChannel },
    BootstrapApprovalDecided   { stage: BootstrapStage, decision: BootstrapDecision,
                                 comment: Option<String> },
    BootstrapEditRequested     { stage: BootstrapStage, feedback: String },

    // ── Pipeline construction ──
    PipelineMaterialized { graph_hash: ContentHash },

    // ── Stage execution ──
    StageEntered          { node: NodeKey, attempt: u32 },
    StageInputsResolved   { node: NodeKey, bindings: BTreeMap<String, ContentHash> },
    SessionOpened         { node: NodeKey, session: SessionId, agent: String },
    ToolCalled            { session: SessionId, tool: String, args_redacted: ContentHash },
    ToolResultReceived    { session: SessionId, success: bool, result: ContentHash },
    ArtifactProduced      { node: NodeKey, artifact: ContentHash, path: PathBuf, name: String },
    OutcomeReported       { node: NodeKey, outcome: OutcomeKey, summary: String },
    StageCompleted        { node: NodeKey, outcome: OutcomeKey },
    StageFailed           { node: NodeKey, reason: String, retry_available: bool },
    SessionClosed         { session: SessionId, disposition: SessionDisposition },

    // ── Routing ──
    EdgeTraversed             { edge: EdgeKey, from: NodeKey, to: NodeKey },
    LoopIterationStarted      { loop_id: NodeKey, item: toml::Value, index: u32 },
    LoopIterationCompleted    { loop_id: NodeKey, index: u32, outcome: OutcomeKey },
    LoopCompleted             { loop_id: NodeKey, completed_iterations: u32, final_outcome: OutcomeKey },

    // ── Human interaction ──
    ApprovalRequested { gate: NodeKey, channel: ApprovalChannel, payload_hash: ContentHash },
    ApprovalDecided   { gate: NodeKey, decision: String, channel_used: ApprovalChannelKind,
                        comment: Option<String> },

    // ── Sandbox ──
    SandboxElevationRequested { node: NodeKey, capability: String },
    SandboxElevationDecided   { node: NodeKey, decision: ElevationDecision, remember: bool },

    // ── Hooks ──
    HookExecuted          { hook_id: String, exit_status: i32, on_failure: HookFailureMode },
    OutcomeRejectedByHook { node: NodeKey, outcome: OutcomeKey, hook_id: String },

    // ── Telemetry ──
    TokensConsumed { session: SessionId, prompt_tokens: u32, output_tokens: u32,
                     cache_hits: u32, model: String, cost_usd: Option<f64> },

    // ── Forking ──
    ForkCreated { new_run: RunId, fork_at_seq: u64 },
}

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapStage { Description, Roadmap, Flow }

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapDecision { Approve, Edit, Reject }

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionDisposition { Normal, AgentCrashed, Timeout, ForcedClose }

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ElevationDecision { Allow, AllowAndRemember, Deny }

#[derive(Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalChannelKind { Telegram, Desktop, Email, Webhook }

pub struct RunConfig {
    pub sandbox_default: SandboxMode,
    pub approval_default: ApprovalPolicy,
    #[serde(default)] pub auto_pr: bool,
}
```

**Design rules** (from RFC-0002):

- Events are append-only; never edited, never deleted.
- Events are complete: payloads contain everything needed to recompute state — no hidden references to mutable external state.
- Events are deterministic: replaying same events on same code produces same result.
- Large content hashed (`ContentHash`), not embedded — file artifacts on disk.
- Sensitive data redacted (tool args containing secrets stored as redacted hash).

**Serialization**: `EventPayload` serializes to **bincode** for the durable event log (compact, fast). `serde` derives are added so that the same type can also serialize to JSON/TOML for debug inspection (e.g., `surge show event` CLI command).

`VersionedEventPayload` is the wire format for the event log file: `schema_version: u32` + payload. Existing code stays on `version=1`. When `EventPayload` changes shape in a backward-incompatible way, we bump version and add a `legacy::EventPayloadV1` historical type with `upgrade_to_v2()` conversion. M1 does not need this yet — it only establishes the wrapping struct.

### 4.20 `run_state.rs`

```rust
use std::sync::Arc;

pub enum RunState {
    NotStarted,
    Bootstrapping {
        stage: BootstrapStage,
        substate: BootstrapSubstate,
    },
    Pipeline {
        /// `Arc<Graph>` because `Graph` is frozen after `PipelineMaterialized` —
        /// every fold step that returns `Pipeline` shares the same graph.
        /// Without `Arc` each fold call would clone tens of KB of nested
        /// `BTreeMap`s on a 50-node run; with `Arc` each step is one atomic
        /// increment. The graph is logically immutable through the entire
        /// pipeline phase, so `Arc` is the correct primitive (not `Rc`,
        /// because `RunState` may cross thread boundaries; not `Cow`, because
        /// we never mutate).
        graph: Arc<Graph>,
        cursor: Cursor,
        memory: RunMemory,
    },
    Terminal {
        kind: TerminalReason,
        reason: String,
    },
}

pub enum BootstrapSubstate {
    AgentRunning   { session: SessionId, started_seq: u64 },
    AwaitingApproval { artifact: ContentHash, requested_seq: u64 },
}

pub struct Cursor {
    pub node: NodeKey,
    pub attempt: u32,
}

#[derive(Copy, PartialEq, Eq)]
pub enum TerminalReason { Completed, Failed, Aborted }

#[derive(Default)]
pub struct RunMemory {
    pub artifacts: BTreeMap<String, ArtifactRef>,                  // by file name
    pub artifacts_by_node: BTreeMap<NodeKey, Vec<ArtifactRef>>,
    pub outcomes: BTreeMap<NodeKey, Vec<OutcomeRecord>>,           // history per node
    pub costs: CostSummary,
}

pub struct ArtifactRef {
    pub hash: ContentHash,
    pub path: PathBuf,
    pub name: String,
    pub produced_by: NodeKey,
    pub produced_at_seq: u64,
}

pub struct OutcomeRecord {
    pub outcome: OutcomeKey,
    pub summary: String,
    pub seq: u64,
}

#[derive(Default)]
pub struct CostSummary {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_hits: u64,
    pub cost_usd: f64,
}
```

**Fold function** (the core of event sourcing):

```rust
/// Apply an event to the run state. Pure function — no I/O, no clock reads.
///
/// Returns either the new state, or a `FoldError` describing why the transition
/// is invalid. Invalid transitions are *not* panics — they may indicate a
/// corrupted event log and should be logged but not crash the engine.
pub fn apply(state: RunState, event: &RunEvent) -> Result<RunState, FoldError>;

/// Apply many events sequentially. Folds from initial state.
pub fn fold(events: &[RunEvent]) -> Result<RunState, FoldError>;

pub enum FoldError {
    InvalidTransition { from: &'static str, event: &'static str },
    CorruptedSequence { expected_seq: u64, got_seq: u64 },
    UnknownNode { node: NodeKey },
    UnknownEdge { edge: EdgeKey },
}
```

`apply` is **exhaustive** over `(RunState, EventPayload)` pairs — every combination either has a transition or returns `FoldError::InvalidTransition`. The match is structured as nested `match` per state variant, with sub-matches for relevant payload variants. Compiler's exhaustiveness check (no `_` arms in either layer) catches missed cases.

`RunMemory::apply_event(&mut self, event: &RunEvent)` is a separate function for accumulating memory state independently — used in tests and for "what's the cost so far at seq N" queries without folding the full state machine.

**Cursor scope**: `Cursor` tracks the current node and attempt number only. Loop iteration state (current iteration index, items remaining) is not part of the cursor — when fold encounters `LoopIterationStarted`, it does not descend into the loop body in M1; the body's execution state lives in the engine, not in `RunState`. M1 fold treats loop iterations as opaque progression markers. Engine-level descent into nested execution comes with the executor in M5.

**Graph sharing**: `Graph` is wrapped in `Arc` from the start. `Arc::clone` is a single atomic increment — folding 1000 events through `RunState::Pipeline` allocates the graph once. The trade-off (consumers see `Arc<Graph>` in their type signatures) is paid up front rather than as a breaking change later when benchmarks would force the migration anyway.

### 4.21 `error.rs` (extension)

Add new variants to existing `SurgeError`:

```rust
pub enum SurgeError {
    // … existing variants stay …

    // ── new in M1 ──

    #[error("Graph validation failed with {0} errors")]
    GraphValidation(Vec<ValidationError>),

    #[error("Event fold failed: {0}")]
    EventFold(#[from] FoldError),

    #[error("Profile parse error: {0}")]
    ProfileParse(String),

    #[error("Content hash mismatch: expected {expected}, got {actual}")]
    ContentHashMismatch { expected: ContentHash, actual: ContentHash },
}
```

These widen the error space but don't change existing variants — pure addition.

## 5. Serialization

### 5.1 TOML — `Graph` and `Profile`

- **Read**: `toml = "0.8"` (already in deps). Standard `serde::Deserialize`.
- **Write**: `toml_edit = "0.22"` (new workspace dep) — preserves comments, blank lines, key ordering on round-trip.

API:

```rust
// graph.rs
impl Graph {
    pub fn from_toml(s: &str) -> Result<Self, GraphParseError>;
    pub fn to_toml(&self) -> Result<String, GraphParseError>;
    /// Edit-aware variant: preserves an existing TOML document's formatting,
    /// updating only changed fields. For the editor's "save" path.
    pub fn merge_into_toml(&self, original: &str) -> Result<String, GraphParseError>;
}

// profile.rs — same shape
impl Profile {
    pub fn from_toml(s: &str) -> Result<Self, ProfileParseError>;
    pub fn to_toml(&self) -> Result<String, ProfileParseError>;
    pub fn merge_into_toml(&self, original: &str) -> Result<String, ProfileParseError>;
}
```

`merge_into_toml` is part of the M1 public API but its initial implementation may delegate to plain `to_toml` (effectively re-serializing without preserving original formatting). True comment-and-formatting preservation lands when the editor (M9) needs it; the API surface is defined now so consumers don't have to change later.

### 5.2 Bincode — `EventPayload`

```rust
// run_event.rs
impl VersionedEventPayload {
    pub fn to_bincode(&self) -> Result<Vec<u8>, BincodeError>;
    pub fn from_bincode(bytes: &[u8]) -> Result<Self, BincodeError>;
}
```

`bincode` is added as a workspace dep. Exact major version (1.x vs 2.x) is pinned during implementation to match what `surge-persistence` already uses, so both crates share one version of the dep tree.

### 5.3 JSON for debug

Every type derives `Serialize`/`Deserialize` for JSON via `serde_json` (workspace dep already exists in some sub-crates). No dedicated API — used ad-hoc by debug commands.

## 6. Testing strategy

Three layers, all under `#[cfg(test)]` modules in their respective files:

### 6.1 Unit tests (per type)

For each non-trivial type:
- TOML round-trip: serialize → parse → semantic equality.
- Bincode round-trip (for events).
- `Default` impls produce sensible values.
- Display/Debug formatting for IDs and hashes.

### 6.2 Property-based tests (`proptest`)

- **Graph round-trip**: generate random valid `Graph` via custom strategy → serialize TOML → parse → compare. Asserts no information loss.
- **Validation**: generate random graphs, run `validate`, assert that valid-by-construction graphs pass and seeded-invalid graphs fail with expected error kind.
- **Fold determinism**: generate random valid event sequences → fold → assert `apply` is associative-ish (`fold(events) == apply(fold(prefix), suffix)`).
- **Outcome routing**: 100 random valid graphs with synthetic outcomes → assert `validate` accepts them and `resolve_next` (helper that follows an outcome to next node) returns expected target.

`proptest = "1"` — new dev-dep workspace-wide.

### 6.3 Snapshot tests (`insta`)

- Handcrafted fixture `flow.toml` files (one per archetype: linear, with-loop, with-subgraph, bug-fix-flavored, refactor-flavored) → parse → snapshot the resulting `Graph` debug output.
- Validation error messages: snapshot the human-readable formatting of each `ValidationErrorKind`.
- Folded state at chosen seq points: handcrafted event sequence → snapshot the `RunState` after fold.

`insta = "1"` — new dev-dep workspace-wide.

Test fixtures live in `crates/surge-core/tests/fixtures/`:

```
tests/fixtures/
├── graphs/
│   ├── linear-trivial.toml             # 3 nodes, no loops
│   ├── linear-with-review.toml         # 5-7 nodes, no loops
│   ├── single-milestone-loop.toml      # 1 outer node + 1 subgraph
│   ├── nested-milestone-loop.toml      # 2 levels of subgraphs
│   ├── nested-3-levels.toml            # 3 levels: milestone-loop → task-loop → impl-step. Validates flat-subgraph design holds for deepest realistic case.
│   ├── bug-fix-flow.toml               # bug-fix archetype
│   ├── refactor-flow.toml              # refactor archetype
│   └── domain-key-real-roadmap.toml    # imported from a real project's roadmap (e.g., domain-key crate's planning doc) — catches issues that synthetic graphs miss
├── profiles/
│   ├── implementer.toml
│   ├── reviewer.toml
│   └── architect.toml
└── events/
    ├── linear-run-success.events.json     # human-editable for fixtures
    └── nested-loop-with-failure.events.json
```

### 6.4 Benchmarks (`criterion`)

Performance budgets are not aspirational — they're acceptance criteria. `criterion` is added as a dev-dep; benchmarks live in `crates/surge-core/benches/`.

| Benchmark | Budget (M3 Pro / Ryzen 7 baseline) | Rationale |
|---|---|---|
| `fold_1k_events_typical_graph` | < 50 ms | 1000-event run on a 10-node graph; this is a typical 30-min run replay |
| `fold_10k_events_typical_graph` | < 500 ms | Heavy run; bound for replay scrubber UX |
| `validate_50_node_graph` | < 5 ms | Editor's live validation — must feel instant on every keystroke |
| `validate_pathological_100_nodes_5_subgraphs` | < 50 ms | Worst case the editor needs to handle |
| `toml_roundtrip_typical_flow` | < 20 ms | Editor save path |
| `bincode_roundtrip_event` | < 10 µs | 30-event burst per second is plausible during heavy work |
| `graph_clone` (for comparison) | reported, no budget | informational — quantifies the `Arc` decision |

If any benchmark exceeds its budget at the end of M1, the milestone is not done — either the implementation needs fixing or the budget needs explicit relaxation with rationale.

## 7. Workspace dependency additions

Added to `Cargo.toml` `[workspace.dependencies]`:

```toml
chrono     = { version = "0.4", features = ["serde"] }
domain-key = "..."                               # latest published
toml_edit  = "0.22"
sha2       = "0.10"
hex        = "0.4"
bincode    = "1.3"
semver     = { version = "1", features = ["serde"] }
proptest   = "1"
insta      = "1"
criterion  = "0.5"
```

`surge-core/Cargo.toml` adds:

```toml
[dependencies]
serde      = { workspace = true }
toml       = { workspace = true }
ulid       = { workspace = true }
thiserror  = { workspace = true }
chrono     = { workspace = true }
domain-key = { workspace = true }
toml_edit  = { workspace = true }
sha2       = { workspace = true }
hex        = { workspace = true }
bincode    = { workspace = true }
semver     = { workspace = true }

[dev-dependencies]
proptest   = { workspace = true }
insta      = { workspace = true }
criterion  = { workspace = true }
```

## 8. Migration impact on other crates

**M1 changes nothing in other crates.** Pure addition, no breaking changes.

Future migration touchpoints (out of scope for M1):

| Consumer | What changes later | When |
|---|---|---|
| `surge-orchestrator` | New `executor` module that consumes `RunEvent` and drives `Graph` (parallel to existing FSM-based code) | M5 (engine milestone) |
| `surge-persistence` | New event log table for `VersionedEventPayload`; legacy `SurgeEvent` table stays | M2 (storage milestone) |
| `surge-cli` | New `run` / `attach` / `fork` commands using `Graph` | M6 |
| `surge-spec` | `flow.toml` parser becomes the new spec writer; legacy `Spec` parser stays | gradual through M5/M6 |

## 9. Acceptance criteria

The milestone is complete when **all** of the following pass:

1. `cargo build -p surge-core` succeeds on Linux, macOS, Windows.
2. `cargo test -p surge-core` passes — including all unit, proptest, and snapshot tests.
3. `cargo clippy -p surge-core --all-targets -- -D warnings` clean.
4. `cargo fmt --all -- --check` clean.
5. All existing `surge-core` tests still pass (no regressions in legacy modules).
6. Other crates in the workspace build unchanged: `cargo build --workspace` succeeds.
7. Existing crates that depend on `surge-core` (`surge-orchestrator`, `surge-persistence`, `surge-cli`, `surge-spec`) compile without code changes.
8. **Behavioral smoke test**: `surge run` against a fixture project produces the same artifacts and final state before and after M1 (legacy code path unaffected). This is a required test, not aspirational.
9. TOML round-trip for `Graph`: serialize → parse → semantic equality for all 8 fixture graphs (including 3-level nested and the imported real-world fixture).
10. Bincode round-trip for `EventPayload`: every variant survives a round-trip.
11. Validation: each of the 17 structural rules + 2 warnings has at least one failing fixture and one passing fixture, both verified by tests. Subgraph reference cycle detection, NodeKey collision detection, and orphan-subgraph warning each have a dedicated fixture.
12. Fold function: handcrafted event sequence of 50+ events folds to the expected `RunState`; this is captured in a snapshot test.
13. Property test: 1000 generated valid graphs all pass validation; 1000 generated invalid graphs all fail with at least one `ValidationError`.
14. `Profile.extends` field round-trips through TOML; an explicit test confirms it is **not** resolved (no registry lookup, no transitive merging) — locking the M1 boundary.
15. All benchmarks in §6.4 meet their budget on the baseline machine (or have an explicit signed-off rationale for relaxation). `cargo bench -p surge-core` produces output that proves it.
16. `surge-core` public API (post-M1) is documented with `///` rustdoc on every public type and function. `cargo doc -p surge-core --no-deps` produces no warnings.

## 10. Out of scope

Explicitly **not** part of M1:

- Profile inheritance resolution (`extends` field). Parsed but not merged.
- Loop body / subgraph execution semantics (engine concern, M5).
- Materialized views over events (storage concern, M2).
- TOML round-trip preservation of comments — best effort only, full preservation is M9 (editor) territory.
- Schema migrations for `EventPayload` — only `VersionedEventPayload` wrapper exists; no v1→v2 path needed yet.
- `RunState::fold` for `Subgraph` / nested `Loop` execution — fold treats them as opaque transitions in M1; nested execution is engine work.
- Mock ACP agent — testing utility, lives in `surge-testing` crate (not yet exists; created later).

## 11. Realistic effort estimate

The new vision's `ROADMAP.md` budgets M1 at 2 weeks. With 19 new files, 30+ event variants, 15 validation rules, property tests, snapshot tests, criterion benchmarks, plus verification across 4 dependent crates and the smoke-test acceptance, **realistic estimate is 3–4 weeks of solo evening/weekend work** (the original ROADMAP voice). Calendar planning should use the upper bound; under-estimating cascades into the rest of the milestone chain.

If the implementer hits one of these unknowns hard (subgraph reference cycle detector turns out tricky; `toml_edit` integration is messier than expected; `domain-key` published version doesn't quite match the API used here) — that's another half-week per surprise. Build buffer in.

## 12. Open questions for implementation phase

These will surface during writing-plans / actual implementation; not blockers for design approval:

- **`domain-key` exact published version.** Author maintains the crate; pin to whatever is current at implementation time. If the published API differs from what this spec assumes (e.g., macro signatures), reconcile during M1.T1.1 and amend this spec.
- **`bincode` 1.x vs 2.x.** Pin to whatever `surge-persistence` already uses to share one version of the dep tree.
- **`toml_edit` serialize integration.** Per §5.1, `merge_into_toml` may initially delegate to `to_toml`. Real comment-preservation logic is M9 territory.
- **`InspectorUiField.kind` extensibility.** May need more variants (e.g., `Path`, `Color`) as we build the editor in M9. Closed enum for now; add variants when needed without breaking changes.
