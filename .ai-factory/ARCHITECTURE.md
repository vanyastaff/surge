# Architecture: Modular Monolith (Rust Workspace) with Hexagonal Influences

## Overview

Surge is a **Rust Cargo workspace organized as a modular monolith with hexagonal (ports-and-adapters) influences**. The workspace is the deployment unit; the 12 member crates are the modules. A leaf domain crate (`surge-core`) holds pure types with no I/O; adapter crates wrap each external concern (SQLite, git, ACP, MCP, notification channels, tracker APIs); two long-running binaries (`surge-cli`, `surge-daemon`) and one UI binary (`surge-ui`) compose those adapters into runnable products.

This pattern was chosen because it matches what surge actually is: a single user-facing tool with clear internal seams. We get strong module boundaries (separate crates that cannot accidentally reach into one another's privates), an explicit dependency graph (cargo refuses cycles, so the rule is enforced by the compiler), and a single deployment surface (one daemon, one CLI, one UI on a developer's machine). The trade-offs that come with microservices — network hops, distributed transactions, polyglot persistence, ops complexity — are non-goals (see `docs/ARCHITECTURE.md` § 13). Multi-user collaboration on the same run is also a non-goal, so a monolith is the correct shape.

## Decision Rationale

- **Project type:** Local-first AFK AI coding orchestrator. Single user, single machine, single deployment unit.
- **Tech stack:** Rust 2024 edition workspace. `tokio` async runtime. SQLite via `rusqlite`. ACP via `agent-client-protocol`. Cargo as the only build system.
- **Domain complexity:** Medium–high. Multiple bounded contexts (orchestrator, ACP bridge, persistence, intake, notification, MCP, UI) but no separate user populations or tenancy.
- **Scale requirements:** One user per machine. No horizontal scaling; vertical scaling only.
- **Team size:** Small. The architecture must be navigable by a single contributor without architectural sprawl.
- **Key factor:** **The agent runtime owns the sandbox**, the tracker owns ticket state, the user owns the machine. Surge sits between them as orchestration only — there is no business logic worth extracting into separate services. A single Rust workspace with strict crate boundaries gives the structure of a microservice mesh without any of its ops cost.

## Folder Structure

```
.
├── Cargo.toml                              # Workspace manifest: members + [workspace.dependencies]
├── crates/
│   │
│   │   ── Domain layer (no I/O) ──────────────────────────────────────────────
│   ├── surge-core/                         # Leaf: graph, profile, event, sandbox, validation types
│   │   ├── src/
│   │   │   ├── lib.rs                      # Public API surface; re-exports submodules
│   │   │   ├── graph.rs                    # Graph type + invariants
│   │   │   ├── node.rs                     # Closed NodeKind enum
│   │   │   ├── edge.rs                     # Edge + EdgeKind (Forward / Backtrack / Escalate)
│   │   │   ├── event.rs / run_event.rs     # Event payloads (bincode-serialized)
│   │   │   ├── state.rs / run_state.rs     # State machines and folds
│   │   │   ├── profile.rs                  # Profile/role configuration (parent)
│   │   │   ├── profile/                    # registry / merge / inheritance / bundled
│   │   │   │   ├── keyref.rs               #   `name@version` parser → ProfileKeyRef
│   │   │   │   ├── registry.rs             #   ResolvedProfile, merge_chain, collect_chain
│   │   │   │   └── bundled.rs              #   17 bundled profiles via include_str!
│   │   │   ├── bundled_flows.rs            # Bundled flow registry via include_str!
│   │   │   ├── sandbox.rs                  # SandboxIntent / launch profiles
│   │   │   ├── validation.rs               # Graph invariants
│   │   │   ├── error.rs                    # SurgeError (thiserror, #[non_exhaustive])
│   │   │   ├── id.rs / keys.rs             # ULID-based IDs, NodeKey / OutcomeKey
│   │   │   └── *_config.rs                 # One config struct per file
│   │   ├── bundled/profiles/               # 17 *.toml assets baked in via include_str!
│   │   ├── bundled/flows/                  # First-party flow.toml assets baked in via include_str!
│   │   └── benches/                        # criterion benches (harness = false)
│   │
│   │
│   │   ── Application layer (orchestrator / engine) ──────────────────────────
│   ├── surge-orchestrator/                 # Engine: graph executor + bootstrap chain + roadmap-amendment surfaces
│   │   ├── src/
│   │   │   ├── prompt.rs                   # PromptRenderer — Handlebars wrapper (strict + lenient)
│   │   │   ├── archetype_registry.rs       # User/bundled flow template lookup
│   │   │   ├── bootstrap_driver.rs         # Bootstrap graph runner → materialized follow-up graph
│   │   │   ├── profile_loader/             # Disk + bundled resolution (I/O-touching)
│   │   │   │   ├── paths.rs                #   surge_home() / profiles_dir() honouring SURGE_HOME
│   │   │   │   ├── disk.rs                 #   DiskProfileSet::scan(*.toml, warn-and-skip)
│   │   │   │   └── registry.rs             #   ProfileRegistry::{load, resolve, list} 3-way lookup
│   │   │   └── engine/                     # graph executor + stage handlers + hooks
│   │
│   │   ── Adapter layer (one external concern per crate) ─────────────────────
│   ├── surge-persistence/                  # SQLite event log, content-addressed artifacts, views, memory
│   ├── surge-acp/                          # ACP bridge (dedicated OS thread, !Send futures)
│   ├── surge-git/                          # git2-based worktree and branch lifecycle
│   ├── surge-intake/                       # TaskSource trait + GitHub Issues / Linear impls
│   ├── surge-mcp/                          # stdio MCP server lifecycle
│   ├── surge-notify/                       # Desktop, webhook, Slack, email, Telegram channels
│   │
│   │   ── Driver layer (binaries / UIs) ──────────────────────────────────────
│   ├── surge-daemon/                       # Long-running engine host (Unix sockets / Windows pipes)
│   ├── surge-cli/                          # `surge` binary (clap-derived command tree)
│   └── surge-ui/                           # GPUI desktop shell
│
├── examples/                               # Sample flow.toml files (engine smoke tests)
├── docs/                                   # docs/ARCHITECTURE.md is the canonical architecture doc
└── .ai-factory/                            # AI-agent context (this file lives here)
```

The folder structure is the **existing** workspace layout — this document codifies the rules already encoded in `Cargo.toml`'s `[workspace.dependencies]` and the per-crate `Cargo.toml` files; it does not propose a reshuffle.

## Dependency Rules

Dependencies flow strictly downward through four conceptual layers. Cargo's cycle detector enforces the absence of back-edges; this section names the layers so contributors can read the rule without spelunking through `Cargo.toml`.

**Layer 1 — Domain (leaf):**
- `surge-core` depends on nothing except external types crates (`serde`, `thiserror`, `chrono`, `ulid`, `bincode`, `toml_edit`, `sha2`, `hex`, `semver`, `humantime-serde`).
- **No I/O. No `tokio`. No filesystem, network, database, process, or thread primitives.** Pure types and folds.

**Layer 2 — Application:**
- `surge-orchestrator` depends on `surge-core` and adapter crates (`surge-persistence`, `surge-acp`, `surge-git`, `surge-mcp`, `surge-intake`, `surge-notify`).
- Owns the engine state machine, hook execution, and the bootstrap sequence.

**Layer 3 — Adapters:**
- Each adapter crate depends on `surge-core` (for shared types) and on its own external dependency family — `surge-persistence` on `rusqlite` / `r2d2`, `surge-acp` on `agent-client-protocol`, `surge-git` on `git2`, `surge-mcp` on `rmcp`, `surge-notify` on `notify-rust` / `lettre` / `tiny_http` / `teloxide` (when wired), `surge-intake` on `octocrab` / `lineark-sdk`.
- **Adapters do not depend on each other.** When two adapters need to coordinate, the orchestrator wires them through `surge-core` types.

**Layer 4 — Drivers (binaries):**
- `surge-cli`, `surge-daemon`, `surge-ui` may depend on any lower layer.
- These are the **only crates allowed to use `anyhow`**. Everything below uses `thiserror`-derived enums.

Allowed and forbidden directions:

- ✅ `surge-cli` → `surge-orchestrator` → `surge-persistence` → `surge-core`
- ✅ `surge-acp` → `surge-core` (shared types)
- ✅ `surge-orchestrator` → `surge-acp` (use the bridge)
- ❌ `surge-core` → anything in surge (it is leaf)
- ❌ `surge-acp` → `surge-persistence` (adapters do not see each other)
- ❌ `surge-persistence` → `surge-orchestrator` (adapters do not depend on the application layer)
- ❌ Any crate → `surge-cli` / `surge-daemon` / `surge-ui` (drivers are top of the graph)
- ❌ Cyclic edges anywhere (cargo refuses to compile; do not work around with re-exports)

## Module Communication

Modules communicate through a small, fixed set of patterns. Reach for a new pattern only when none of these fits — that is a design discussion, not a tactical decision.

1. **Plain function calls across crate boundaries** — the default. Public types from one crate consumed by another. Stable trait surfaces are the seams; concrete types live behind `pub` modules.

2. **Trait objects for ports (hexagonal seams).** Examples:
   - `TaskSource` in `surge-intake` — implementations for GitHub Issues and Linear; the orchestrator depends on the trait, not the impl.
   - The notification channel trait in `surge-notify` — desktop / Slack / email / Telegram all behind one shape.
   - The agent registry / pool traits in `surge-acp` — Claude Code, Codex, Gemini, mock agent all interchangeable.
   New external systems are wired by adding an impl to an existing port, not by adding a new crate.

3. **Tokio channels for crossing thread / runtime boundaries.** The ACP bridge is `!Send` and runs on a dedicated OS thread with a single-threaded `tokio::runtime::Builder` + `LocalSet`. The engine talks to it via:
   - `mpsc::Sender<BridgeCommand>` — `OpenSession`, `SendMessage`, `CloseSession`.
   - `broadcast::Sender<BridgeEvent>` — `SessionEstablished`, `ToolCall`, `ToolResult`, `PermissionRequest`, `AgentMessageChunk`, `TokensConsumed`, `SessionEnded`.
   This is the canonical pattern for any `!Send` external SDK.

4. **The event log is the wire format between writer and readers.** `surge-daemon` is the **single writer** to the per-run SQLite event log. `surge-cli`, `surge-ui`, the Telegram bot, and any future surface read the same file with WAL-mode SQLite. There is no in-process pub-sub spanning processes; durability and ordering come from SQLite.

5. **Cross-process IPC for client/daemon traffic.** `surge-cli` ↔ `surge-daemon` uses Unix domain sockets on Linux/macOS and named pipes on Windows via the `interprocess` crate. Stdio piping for child processes (`rmcp`'s `transport-child-process`, MCP server lifecycle).

6. **Append-only events as the only source of truth.** State changes inside the engine are first written to the event log; in-memory views are folds. Replay = re-fold from `seq = 0`. Fork = copy events `1..N` into a new run id. Crash recovery = on daemon start, scan non-terminal runs and re-fold each.

## Key Principles

1. **Engine is dumb, agents are smart.** Routing decisions are graph data (declarative edges keyed by outcome). The LLM does the work; it does not pick the next node. The closed `NodeKind` enum and dynamic-per-node outcome enum on the `report_stage_outcome` injected tool make this enforceable at compile time on one side and at the SDK boundary on the other.

2. **`surge-core` is a leaf.** Anything in `surge-core` must be folded, validated, and tested without touching the OS. Adding a `tokio` dependency or a `std::fs` call to `surge-core` is a refactor against the architecture, not a feature.

3. **Sandbox is delegated; surge never reimplements OS isolation.** `SandboxIntent` is a typed value passed to the agent runtime. Surge observes elevation requests and writes `SandboxElevationRequested` events; the runtime's native sandbox does the enforcing. No Landlock / sandbox-exec / AppContainer code in this tree.

4. **Closed enums for kinds; open traits for ports.** `NodeKind`, `EdgeKind`, terminal outcome codes, and run statuses are closed enums — extensibility happens via profiles, named agents, and templates, not new variants. Conversely, plug-points (task sources, notification channels, agent providers) are traits with open implementation sets.

5. **Determinism in folds.** Event folding never reads the wall clock, never generates random IDs, and never depends on `HashMap` iteration order. Use `BTreeMap` or ordered `Vec<(K, V)>` in serialized payloads. Replay must reproduce identical state byte-for-byte.

6. **One git worktree per run.** The worktree is created by `surge-git` at run start, lives under `~/.surge/runs/<run_id>/worktree/`, and is merged or discarded on terminal outcome. Local in-progress branches use the project's `.worktrees/` convention; CI / the user's editor never see a half-merged tree.

7. **`anyhow` only at the binary boundary.** Library crates expose typed errors via `thiserror`; binaries flatten them into `anyhow::Result` for the human-facing surface. A `use anyhow::*` import inside `crates/surge-core/` (or any non-binary crate) is a regression.

## Code Examples

### Adapter trait (port) and an implementation

The `TaskSource` port lives in `surge-intake` and is implemented once per tracker. The orchestrator never sees `octocrab` or `lineark_sdk` directly.

```rust
// crates/surge-intake/src/lib.rs

use async_trait::async_trait;
use surge_core::{TaskMeta, TaskSourceError, TaskUri};

/// A source of work — GitHub issues, Linear issues, future Discord/Jira/Slack/Notion.
/// Implementors live alongside this trait under `crates/surge-intake/src/sources/`.
#[async_trait]
pub trait TaskSource: Send + Sync {
    /// Stable identifier for the source kind, e.g. `"github"`, `"linear"`.
    fn kind(&self) -> &'static str;

    /// Fetch and normalize a task into surge-core types. The tracker is the
    /// master; this method only reads.
    async fn fetch(&self, uri: &TaskUri) -> Result<TaskMeta, TaskSourceError>;

    /// Write surge-controlled labels (`surge:enabled`, `surge:auto`,
    /// `surge:template/<name>`, `surge-priority/<level>`) and progress comments.
    /// Tracker status (open/closed/in-progress) stays under the user's control.
    async fn annotate(&self, uri: &TaskUri, ann: TaskAnnotation) -> Result<(), TaskSourceError>;
}

// crates/surge-intake/src/sources/github.rs

pub struct GitHubTaskSource {
    client: octocrab::Octocrab,
}

#[async_trait]
impl TaskSource for GitHubTaskSource {
    fn kind(&self) -> &'static str { "github" }

    async fn fetch(&self, uri: &TaskUri) -> Result<TaskMeta, TaskSourceError> {
        // octocrab calls happen here; the trait surface keeps them invisible
        // to the orchestrator.
        // ...
    }

    async fn annotate(&self, uri: &TaskUri, ann: TaskAnnotation) -> Result<(), TaskSourceError> {
        // ...
    }
}
```

### Bridge command/event channel (crossing the `!Send` boundary)

```rust
// crates/surge-acp/src/bridge.rs

use tokio::sync::{broadcast, mpsc};

pub enum BridgeCommand {
    OpenSession { profile: ProfileId, sandbox: SandboxIntent, reply: oneshot::Sender<SessionId> },
    SendMessage { session: SessionId, payload: AgentMessage },
    CloseSession { session: SessionId },
}

#[derive(Clone, Debug)]
pub enum BridgeEvent {
    SessionEstablished { session: SessionId },
    ToolCall { session: SessionId, call: ToolCall },
    ToolResult { session: SessionId, result: ToolResult },
    PermissionRequest { session: SessionId, request: PermissionRequest },
    AgentMessageChunk { session: SessionId, chunk: MessageChunk },
    TokensConsumed { session: SessionId, usage: TokenUsage },
    SessionEnded { session: SessionId, reason: EndReason },
}

pub struct BridgeHandle {
    cmd_tx: mpsc::Sender<BridgeCommand>,
    evt_tx: broadcast::Sender<BridgeEvent>,
}

/// Spawn the dedicated OS thread that owns the !Send ACP runtime.
/// The orchestrator only ever sees this Send/Sync handle.
pub fn spawn_bridge() -> BridgeHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<BridgeCommand>(64);
    let (evt_tx, _)      = broadcast::channel::<BridgeEvent>(256);
    let evt_tx_for_thread = evt_tx.clone();

    std::thread::Builder::new()
        .name("surge-acp-bridge".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("acp bridge runtime");
            let local = tokio::task::LocalSet::new();
            rt.block_on(local.run_until(run_bridge_loop(cmd_rx, evt_tx_for_thread)));
        })
        .expect("spawn acp bridge thread");

    BridgeHandle { cmd_tx, evt_tx }
}
```

### Typed errors in libraries; `anyhow` only in binaries

```rust
// crates/surge-core/src/error.rs — library crate
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SurgeError {
    #[error("graph validation failed: {0}")]
    Validation(#[from] ValidationError),

    #[error("event payload schema {found} is older than supported minimum {min}")]
    SchemaTooOld { found: u32, min: u32 },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

// crates/surge-cli/src/main.rs — binary crate
use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    cli.run().context("surge cli")?;
    Ok(())
}
```

### Deterministic event folding

```rust
// crates/surge-core/src/run_state.rs

/// Fold an event into the run state. No wall-clock reads, no random IDs.
/// Replaying the log produces byte-identical state.
#[must_use]
pub fn fold(state: RunState, event: &RunEvent) -> RunState {
    match &event.payload {
        EventPayload::RunStarted { run_id, started_at } => state.start(*run_id, *started_at),
        EventPayload::StageEntered { node, .. }         => state.enter(node.clone()),
        EventPayload::OutcomeReported { outcome, .. }   => state.record_outcome(outcome.clone()),
        EventPayload::EdgeTraversed { to, .. }          => state.advance_to(to.clone()),
        EventPayload::RunCompleted { ended_at }         => state.complete(*ended_at),
        // ...
    }
}
```

## Anti-Patterns

- ❌ **Adding a `tokio` dependency or any I/O to `surge-core`.** It is leaf; if you need async there, the design is wrong — push the I/O up to an adapter or down into a function the orchestrator calls.
- ❌ **Cross-adapter direct dependencies** (e.g., `surge-acp` depending on `surge-persistence`). Adapters do not see each other; orchestration goes through `surge-orchestrator` using `surge-core` types.
- ❌ **Cyclic dependencies between crates.** Cargo blocks this; do not work around it with re-exports or a "shared" sub-crate that grows into a god module.
- ❌ **`unwrap()` / `expect()` / `dbg!` / `println!` in library code.** Tests and `const` contexts are exempt (allowed via `clippy.toml`). Use the `tracing::*` macros for logs.
- ❌ **`anyhow` outside binary crates.** Library crates expose `thiserror`-derived error enums.
- ❌ **Reaching into a child crate's privates from a sibling.** If you need a type, make it `pub` from `surge-core` (or add a re-export there). Do not depend on `crate::internal::Foo` from another crate's path — the privacy boundary is the seam we are paying for.
- ❌ **Adding a new `NodeKind` variant for a new feature.** The enum is closed; add a profile, named agent, or template instead.
- ❌ **Reimplementing OS sandboxing (Landlock, sandbox-exec, AppContainer).** The agent runtime owns this. Surge passes intent and observes elevation requests.
- ❌ **Wall-clock reads or random ID generation inside `fold`.** Folds must be deterministic — replay produces identical state.
- ❌ **`HashMap` in serialized event payloads.** Iteration order is non-deterministic; use `BTreeMap` or `Vec<(K, V)>`.
- ❌ **Telegram / Slack / email logic outside `surge-notify`.** Notification channels live behind one trait in one crate. New channels are new impls, not new crates.
- ❌ **A second user-facing config format alongside `surge.toml`.** If you need new config, extend the existing schema and update `surge.example.toml`.
