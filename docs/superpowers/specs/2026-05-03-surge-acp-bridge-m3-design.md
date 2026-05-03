# M3 — `surge-acp` bridge for vibe-flow ACP integration

**Status:** Design
**Date:** 2026-05-03
**Predecessor:** [M2 — surge-persistence storage layer](2026-05-02-surge-persistence-m2-design.md)
**Related architecture docs:** `docs/revision/04-acp-integration.md`, `docs/revision/03-engine.md`, `docs/revision/0006-sandbox-and-approvals.md`

---

## 1. Goal

Add a vibe-flow-shaped ACP bridge to `surge-acp` as a new `bridge` submodule, alongside the existing legacy stack (`AgentPool`, `AgentConnection`, `SurgeClient`, `Registry`, `AgentDiscovery`, `HealthTracker`, `AgentRouter`). Pure-addition strategy, same as M1 and M2.

This milestone unblocks M5 (engine), which needs to:

- Open and close ACP sessions per `Stage` execution, addressed by a `surge_core::SessionId`.
- Receive a typed event stream (`BridgeEvent`) it can fold into `EventPayload::SessionOpened`/`ToolCalled`/`ToolResultReceived`/`TokensConsumed`/`SessionClosed`/`ApprovalRequested` and append to the per-run event log via `RunWriter` from M2.
- Inject two engine-owned tools into every session: `report_stage_outcome` (with dynamic outcome enum derived from the node's `OutcomeDecl`s) and `request_human_input`.
- Filter the visible MCP/built-in tool surface through a `Sandbox` so the agent literally cannot call disallowed tools.
- Detect agent subprocess crashes within ~2 s and surface them as `SessionEndReason::AgentCrashed`.
- Run multiple concurrent sessions across runs without interference.

### 1.1 In scope

- `surge-acp::bridge` submodule:
  - `AcpBridge` — owned by the engine, spawns its own dedicated OS thread running a current-thread tokio runtime + `LocalSet` for `!Send` ACP futures.
  - `BridgeCommand` (mpsc) for `OpenSession`/`SendMessage`/`CloseSession`/`GetSessionState`/`Shutdown`.
  - `BridgeEvent` (broadcast) for `SessionEstablished`/`AgentMessage`/`ToolCall`/`ToolResult`/`SessionEnded`/`Error`/`TokenUsage`.
  - `SessionConfig` — flat, profile-derived input passed in by the engine; bridge does not import `Profile`.
- Engine-injected tools registered with each session:
  - `report_stage_outcome` with an `outcome.enum` populated from `Vec<OutcomeKey>` at open time.
  - `request_human_input`, gated by a per-session `allows_escalation: bool` flag.
- `Sandbox` trait (interim M3 surface) with `AlwaysAllowSandbox` and `DenyListSandbox` initial impls; tier-3 OS enforcement deferred to M4.
- Crash detection: bridge waits on `Child::wait()` for each session subprocess and emits `BridgeEvent::SessionEnded { reason: AgentCrashed }` within 2 s of process exit.
- Token usage extraction from ACP `SessionUpdate` carrying `unstable_session_usage` metadata; bridge emits `BridgeEvent::TokenUsage`.
- Mock ACP agent binary (`mock-acp-agent`) for deterministic integration tests, behavior driven by env vars and CLI args.
- `tracing` instrumentation: per-bridge `info_span!("acp_bridge")`, per-session `info_span!("acp_session", session = %sid)`, per-tool-call `debug_span!("tool_call", tool, call_id)`.

### 1.2 Out of scope (deferred to later milestones)

- Engine-side wiring (BridgeEvent → RunEvent, Storage append, hooks, sandbox elevation flow). M5 owns this.
- Tier-3 OS sandbox enforcement (Landlock, sandbox-exec, AppContainer). M4 owns this — M3 ships only the `Sandbox` trait + filtering layer (tier 1 of RFC-0006).
- Real MCP server protocol (`stdio` MCP servers, capability discovery, JSON-RPC). M3 treats MCP tool definitions as a flat `Vec<ToolDef>` supplied by the engine — full MCP integration lives in a later milestone.
- Telegram approval flow for `request_human_input`. M3 emits the approval request as a `BridgeEvent`; routing to Telegram is M7.
- Migration of the legacy `AgentPool` consumers (orchestrator, CLI `surge run`) onto the new bridge. Legacy stays operational, untouched.
- Cost-USD calculation. M3 carries token counts and cache-hit counts only; USD conversion lives in `surge-persistence::runs::views::cost_summary` (already designed in M2) which receives the values via `EventPayload::TokensConsumed`.
- Replay/scrubber UI integration. M11.

## 2. Strategy

### 2.1 Pure addition inside `surge-acp`, no new crate

Per the migration-strategy decision (no parallel "v1"/"v2" crates), all M3 code lives inside `surge-acp` as a `bridge` submodule. Legacy modules (`client`, `connection`, `pool`, `discovery`, `display`, `health`, `process_tracker`, `registry`, `router`, `secrets`, `terminal`, `transport`) remain operational and untouched. The legacy `AgentPool` already uses a `LocalSet` worker for the same `!Send` reason; we do not share that worker with the new bridge — see §2.3.

### 2.2 Reuse `agent-client-protocol = 0.10.2` and existing transport

`agent-client-protocol` 0.10.2 is already in workspace deps with `unstable_session_usage` enabled. The bridge reuses:

- `crate::transport::{StdioTransport, TcpTransport, AgentTransport}` — process spawn + ACP I/O setup.
- `crate::client::SurgeClient` as the **starting point** for the bridge-side ACP `Client` impl. M3 introduces a new client type `BridgeClient` purpose-built for the bridge (event emission via `BridgeEvent`, not `SurgeEvent`); it borrows the file-path validation, secrets redaction, and terminal-handling helpers from `SurgeClient` via shared internal helpers extracted into `crate::shared::*` (small private refactor — public API of `SurgeClient` unchanged).
- `crate::secrets::redact_secrets` — applied to tool-call args before they enter `BridgeEvent::ToolCall`.

### 2.3 Bridge owns its own LocalSet thread

The legacy `AgentPool::worker` thread runs its own `LocalSet`. The bridge spawns a **separate** OS thread with a fresh current-thread tokio runtime + `LocalSet`. Reasons:

1. **Lifecycle independence.** Legacy `AgentPool` and the new `AcpBridge` are owned by different consumers (legacy orchestrator vs vibe-flow engine). Sharing one worker would couple their lifetimes — dropping the bridge would have to consider whether legacy callers are still using the same worker.
2. **State isolation.** Legacy `WorkerState` (connections keyed by agent name, health, resilience config) is structurally different from `BridgeState` (sessions keyed by `SessionId`, tool injection per session, sandbox per session). Trying to share would force a union type.
3. **Backpressure independence.** `mpsc::channel(64)` for the bridge can fill up under heavy session traffic without blocking legacy ping/prompt commands or vice versa.

The cost is one extra OS thread per surge process. That is negligible (<1 MB stack, tokio runtime overhead a few hundred KB).

### 2.4 No engine integration in M3

The bridge emits `BridgeEvent`s on a `tokio::sync::broadcast` channel. M3 does **not** consume those events into `RunWriter` from M2; that wiring belongs to M5 (engine). M3 ships:

- The bridge interface that M5 will call.
- Concrete event payloads that M5 will translate into `EventPayload`.
- Mock ACP agent that M5 (and earlier debugging) can drive end-to-end without a real Claude/Codex/Gemini.

This keeps M3's blast radius limited and the milestone independently testable.

### 2.5 Sandbox: minimal trait now, real enforcement in M4

M3 introduces the `Sandbox` trait with one method binding:

```rust
fn allows_tool(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision;
```

returning `Allow | Deny | Elevate { capability: String }`. The bridge's only enforcement responsibility in M3 is the **tool-list filter at session-open time** (per RFC-0006 §Tier-1: the agent literally doesn't see disallowed tools). This is the cheapest and highest-impact sandbox layer; tiers 2–4 (path checks, OS-level enforcement, network) come in M4.

M3 ships two impls:

- `AlwaysAllowSandbox` — for development and the mock agent.
- `DenyListSandbox { denied_tools, denied_mcp_ids }` — minimal allow-by-default with explicit denylist; useful for the M3 integration tests and as a stepping stone to M4.

`Elevate` is wired through `BridgeEvent::ToolCall` carrying `sandbox_decision: SandboxDecision` so M5 can route to elevation flow. M3 itself does not block on user input.

### 2.6 No traits for mocking the bridge

Per the same rationale as M2 §2.4: concrete types until real test pain demands abstraction. The mock ACP agent in §7 is a separate **process** the bridge talks to over stdio — exactly like a real agent. We don't introduce a `BridgeBackend` trait. If M5 engine accumulates real test pain (>100 ms of subprocess startup per unit test, or platform-specific mock-agent flakiness), introduce `BridgeFacade` traits then.

### 2.7 Mock agent placement

The mock ACP agent ships as a binary target inside `surge-acp` itself (`crates/surge-acp/src/bin/mock_acp_agent.rs`), not a separate `crates/testing` crate. Reasoning:

- It's a small (<400 LOC) test fixture, not reusable infrastructure.
- Tests inside `surge-acp` get it via `cargo build --bin mock_acp_agent` automatically.
- A separate crate would force every consumer (later: `surge-engine`, integration tests across the workspace) to depend on it explicitly. With the binary in `surge-acp`, M5 can locate it via `target/debug/mock_acp_agent` resolved through `env!("CARGO_BIN_EXE_mock_acp_agent")` in tests.

## 3. Module layout

All new files under `crates/surge-acp/src/bridge/`. Legacy modules stay flat at `src/`.

```
crates/surge-acp/src/
├── lib.rs                       (extended re-exports for `bridge::*`)
│
│  ── legacy (no changes in M3) ──
├── client.rs                    SurgeClient (legacy ACP Client impl)
├── connection.rs                AgentConnection
├── discovery.rs                 AgentDiscovery
├── display.rs                   UI models
├── health.rs                    HealthTracker
├── pool.rs                      AgentPool
├── process_tracker.rs           ProcessTracker
├── registry.rs                  Registry, AgentCapability
├── router.rs                    AgentRouter
├── secrets.rs                   redact_secrets (extended in M3 for new bridge use)
├── terminal.rs                  Terminals
├── transport.rs                 StdioTransport, TcpTransport
│
│  ── new (M3) ──
├── shared/
│   ├── mod.rs                   internal helpers shared by SurgeClient and BridgeClient
│   ├── path_guard.rs            worktree-rooted path validation (extracted from client.rs)
│   └── content_block.rs         ContentBlock helpers shared between legacy and bridge
└── bridge/
    ├── mod.rs                   public surface; pub use of below
    ├── acp_bridge.rs            AcpBridge: spawn(), open_session(), send_message(), close_session(), shutdown()
    ├── command.rs               BridgeCommand enum + reply types
    ├── event.rs                 BridgeEvent enum + AgentMessageMeta, SessionEndReason
    ├── session.rs               SessionConfig, AcpSession (internal), SessionState
    ├── client.rs                BridgeClient — Client trait impl that emits BridgeEvent
    ├── tools.rs                 InjectedTool, ToolDef, build_report_stage_outcome_tool, build_request_human_input_tool, dispatch_tool_call
    ├── sandbox.rs               Sandbox trait + AlwaysAllowSandbox + DenyListSandbox + SandboxDecision
    ├── tokens.rs                TokenUsageExtractor — pulls metadata out of ACP SessionUpdate
    ├── error.rs                 BridgeError, AcpError variants
    └── worker.rs                bridge_loop(): the LocalSet body that owns sessions, dispatches commands

crates/surge-acp/src/bin/
└── mock_acp_agent.rs            standalone Mock ACP agent binary (T3.12)

crates/surge-acp/tests/
├── bridge_session_lifecycle.rs   open → message → close round-trip via mock agent
├── bridge_tool_injection.rs      report_stage_outcome dynamic enum + request_human_input
├── bridge_sandbox_filtering.rs   DenyListSandbox prunes tool list before agent sees it
├── bridge_crash_detection.rs     SIGKILL mock agent → SessionEnded{AgentCrashed} ≤2s
├── bridge_concurrent_sessions.rs N parallel sessions, no interference
├── bridge_token_tracking.rs      TokenUsage events accumulate correctly
└── bridge_streaming.rs           AgentMessage chunks delivered in order in real time
```

**Module visibility.** `shared/` is `pub(crate)` — accessible to legacy `client.rs` and to the new `bridge::*`, but **not** exposed in `surge-acp`'s public API. The legacy `pub use client::SurgeClient` line and the new `pub use bridge::*` line are the only public surfaces; downstream consumers (engine, CLI) cannot reach `shared::*`. Implementer note: declare `mod shared;` (not `pub mod shared;`) at `lib.rs`, and `pub(crate)` re-exports inside `shared/mod.rs`.

**Counts.** New code: 1 `shared/` module (3 files, `pub(crate)`), 1 `bridge/` module (11 files), 1 binary, 13 integration tests. Legacy untouched: 13 files. Largest file expected: `worker.rs` (~400 lines: command dispatch, session state machine, child-wait spawning), then `acp_bridge.rs` (~250), `session.rs` (~250), `tools.rs` (~250), `client.rs` (~250 — `Client` trait has 11 methods).

## 4. Public API

### 4.1 Module surface (`bridge::mod`)

```rust
pub use acp_bridge::AcpBridge;
pub use command::BridgeCommand;          // public for tests; not constructed by callers normally
pub use error::{BridgeError, AcpError, OpenSessionError, SendMessageError, CloseSessionError};
pub use event::{
    BridgeEvent, AgentMessageMeta, SessionEndReason, ToolCallMeta, ToolResultPayload,
};
pub use sandbox::{Sandbox, SandboxDecision, AlwaysAllowSandbox, DenyListSandbox};
pub use session::{SessionConfig, SessionState, MessageContent};
pub use tools::{ToolDef, ToolDefSchema, ToolCategory};
```

`BridgeCommand` is exported so test doubles can construct commands manually; production callers use the `AcpBridge` methods.

### 4.2 `AcpBridge`

```rust
pub struct AcpBridge {
    cmd_tx: mpsc::Sender<BridgeCommand>,
    event_tx: broadcast::Sender<BridgeEvent>,        // kept alive so subscribe() works after spawn
    // JoinHandle<()> not Result<...>: a panicking thread cannot return a value;
    // panic payload surfaces via the Err variant of JoinHandle::join().
    worker: Option<std::thread::JoinHandle<()>>,
}

impl AcpBridge {
    /// Spawn the bridge worker thread.
    ///
    /// `cmd_capacity` bounds the command mpsc; `event_capacity` bounds the event
    /// broadcast (lagged subscribers silently drop oldest, like all broadcast channels).
    /// Sane defaults via `AcpBridge::with_defaults()`.
    pub fn spawn(cmd_capacity: usize, event_capacity: usize) -> Result<Self, BridgeError>;
    pub fn with_defaults() -> Result<Self, BridgeError> { Self::spawn(64, 1024) }

    /// Subscribe to the bridge's event stream. Multiple subscribers allowed.
    pub fn subscribe(&self) -> broadcast::Receiver<BridgeEvent>;

    /// Open a new ACP session. The returned SessionId is generated by the bridge
    /// (a new `surge_core::SessionId`); the ACP-side session string is opaque to
    /// callers and lives inside the bridge's SessionState.
    pub async fn open_session(&self, config: SessionConfig)
        -> Result<SessionId, OpenSessionError>;

    /// Send a user message to an open session. Returns once the bridge has
    /// queued the message; agent processing is asynchronous and surfaces via
    /// BridgeEvent::AgentMessage / ToolCall / SessionEnded.
    pub async fn send_message(&self, session: SessionId, content: MessageContent)
        -> Result<(), SendMessageError>;

    /// Read the bridge's view of session state (open/closed/crashed, agent kind, etc.).
    pub async fn session_state(&self, session: SessionId)
        -> Result<SessionState, BridgeError>;

    /// Close a session gracefully. ACP shutdown is sent, child process is awaited
    /// (with a configurable timeout, default 5s); on timeout the child is killed.
    pub async fn close_session(&self, session: SessionId)
        -> Result<(), CloseSessionError>;

    /// Send `Shutdown` to the worker, drain pending commands, await the thread.
    /// All open sessions are forcibly closed (with `SessionEndReason::ForcedClose`
    /// emitted for each). Consumes self; future calls are not possible.
    pub async fn shutdown(self) -> Result<(), BridgeError>;
}
```

`SessionId` here is `surge_core::SessionId` (a ULID newtype from M1 — already in `surge-core::id`).

### 4.3 `SessionConfig`

```rust
pub struct SessionConfig {
    /// Agent flavor — drives subprocess invocation. Reuses crate::registry types.
    pub agent_kind: AgentKind,

    /// Working directory for the agent subprocess. Should be the per-run worktree
    /// path produced by `surge_git::create_run_worktree` (M2). Bridge does not
    /// validate this is a git worktree — that's M5's responsibility.
    pub working_dir: PathBuf,

    /// System prompt sent to the agent in the initial message frame.
    pub system_prompt: String,

    /// Outcome keys that the engine will accept from `report_stage_outcome`.
    /// Bridge derives the JSON-Schema enum from these and injects it as a tool.
    /// Empty Vec is rejected at open time — agents need at least one outcome.
    pub declared_outcomes: Vec<OutcomeKey>,

    /// Whether to inject `request_human_input` tool (for stages that allow
    /// escalation). Drives the boolean check inside tools::build_injected_tools.
    pub allows_escalation: bool,

    /// Engine-supplied list of tools (MCP-flavored or otherwise). Passed through
    /// the sandbox filter before being declared to the agent.
    pub tools: Vec<ToolDef>,

    /// Sandbox to apply to the tool list and to ToolCall events.
    /// Boxed to keep SessionConfig trivially Sized; cloned via Sandbox: Clone bound.
    pub sandbox: Box<dyn Sandbox>,

    /// Permission policy shared with the legacy SurgeClient (auto-approve, smart, …).
    /// In M3 the bridge uses this only for the `Client::request_permission` impl;
    /// the actual sandbox decisions go through `Sandbox::allows_tool`.
    pub permission_policy: PermissionPolicy,

    /// Optional binding labels — opaque key-value pairs the engine attaches to
    /// the SessionConfig for later correlation in BridgeEvents (e.g., the
    /// node_key, the run_id). Bridge passes these through to BridgeEvent::SessionEstablished.
    /// Capped at 8 entries × 64 chars each to bound payload size.
    pub bindings: BTreeMap<String, String>,
}
```

`AgentKind` reuses `crate::registry::DetectedAgent` shape but exposed under a fresh enum to keep bridge API stable independently of registry refactors:

```rust
pub enum AgentKind {
    ClaudeCode { binary: PathBuf, extra_args: Vec<String> },
    Codex      { binary: PathBuf, extra_args: Vec<String> },
    GeminiCli  { binary: PathBuf, extra_args: Vec<String> },
    Custom     { binary: PathBuf, args: Vec<String> },
    /// Used by tests and the mock agent. The bridge knows how to launch
    /// `mock_acp_agent` from CARGO_BIN_EXE_* at compile time.
    Mock       { args: Vec<String> },
}
```

### 4.4 `BridgeCommand`

```rust
pub enum BridgeCommand {
    OpenSession {
        config: SessionConfig,
        reply: oneshot::Sender<Result<SessionId, OpenSessionError>>,
    },
    SendMessage {
        session: SessionId,
        content: MessageContent,
        reply: oneshot::Sender<Result<(), SendMessageError>>,
    },
    SessionState {
        session: SessionId,
        reply: oneshot::Sender<Result<SessionState, BridgeError>>,
    },
    CloseSession {
        session: SessionId,
        reply: oneshot::Sender<Result<(), CloseSessionError>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}
```

`MessageContent`:

```rust
pub enum MessageContent {
    Text(String),
    /// ACP can carry a Vec<ContentBlock> with text, image, audio. M3 supports
    /// text-and-images; audio is forwarded blindly to the agent (no validation).
    Blocks(Vec<agent_client_protocol::ContentBlock>),
}
```

### 4.5 `BridgeEvent`

```rust
pub enum BridgeEvent {
    /// Emitted once after the ACP handshake succeeds and tools are declared.
    SessionEstablished {
        session: SessionId,
        agent: String,                    // human-readable, e.g. "claude-code"
        bindings: BTreeMap<String, String>,
        tools_visible: Vec<String>,       // names after sandbox filter; for observability
    },

    /// Streaming agent output. Multiple events per session for token-by-token
    /// or chunked streaming, depending on agent.
    AgentMessage {
        session: SessionId,
        chunk: String,
        meta: Option<AgentMessageMeta>,
    },

    /// Cumulative token-usage report. Emitted whenever the agent reports new
    /// usage via the `unstable_session_usage` ACP feature, OR at session close
    /// as a final aggregate.
    TokenUsage {
        session: SessionId,
        prompt_tokens: u32,
        output_tokens: u32,
        cache_hits: u32,
        model: String,
    },

    /// Agent invoked a tool. Carries the redacted args (secrets removed) and
    /// the sandbox decision so M5 can route to elevation if Decision::Elevate.
    ToolCall {
        session: SessionId,
        call_id: String,
        tool: String,
        args_redacted_json: String,
        sandbox_decision: SandboxDecision,
        meta: ToolCallMeta,
    },

    /// Tool call result returning to the agent. M5 produces this via
    /// AcpBridge::send_tool_result(call_id, result) in a follow-up command;
    /// in M3 the bridge auto-replies with `unsupported` for non-injected tools.
    /// (See §5.4 for the §M3 bridge auto-reply policy.)
    ToolResult {
        session: SessionId,
        call_id: String,
        payload: ToolResultPayload,
    },

    /// Special engine-injected tool: agent reported its stage outcome.
    /// Surfaced as a distinct event (not a generic ToolCall) so M5 can fold
    /// directly into EventPayload::OutcomeReported without re-parsing args.
    OutcomeReported {
        session: SessionId,
        outcome: OutcomeKey,
        summary: String,
        artifacts_produced: Vec<String>,
    },

    /// Special engine-injected tool: agent asked for human input mid-stage.
    /// M5 will route this to a HumanGate-style flow. The `call_id` is required
    /// because §13.4's future `reply_to_human_input(call_id, payload)` API
    /// needs to correlate the human's reply back to the pending tool call.
    HumanInputRequested {
        session: SessionId,
        call_id: String,
        question: String,
        context: Option<String>,
    },

    /// Session closed for any reason. Final event for that SessionId; subscribers
    /// can free per-session resources after observing this.
    SessionEnded {
        session: SessionId,
        reason: SessionEndReason,
    },

    /// Bridge-level error.
    ///
    /// **Emit conditions** (the exhaustive list — `Error` is not a generic
    /// dumping ground):
    ///
    /// 1. ACP protocol violation that did **not** kill the session (recoverable
    ///    parse failure on a non-critical frame, unknown notification kind, etc.).
    /// 2. Tool dispatch failed but the session continues. In M3 the bridge
    ///    auto-replies `Unsupported` for non-injected tools, so this fires only
    ///    on JSON parse failure of `report_stage_outcome` / `request_human_input`
    ///    args before the bridge can emit `OutcomeReported` / `HumanInputRequested`.
    /// 3. Token extraction failed (malformed `unstable_session_usage` metadata
    ///    that `bridge::tokens` could not decode).
    ///
    /// Errors that **end** the session emit `SessionEnded` instead, not `Error`.
    /// If both apply (the session ends as a direct result of the error), the
    /// bridge emits `Error` first, then `SessionEnded`, so subscribers see the
    /// diagnostic before the terminator.
    Error {
        session: Option<SessionId>,
        error: String,
    },
}

pub struct AgentMessageMeta {
    pub model: Option<String>,
    pub timestamp_ms: i64,
}

pub enum SessionEndReason {
    /// `close_session()` returned successfully or agent volunteered ACP shutdown.
    Normal,
    /// Subprocess exited with non-zero or signal mid-session.
    AgentCrashed { exit_code: Option<i32>, stderr_tail: String },
    /// `close_session()` exceeded its grace timeout; child was killed.
    Timeout { duration_ms: u64 },
    /// `AcpBridge::shutdown()` triggered while session was still open.
    ForcedClose,
}

pub struct ToolCallMeta {
    /// MCP server id that owns this tool, if applicable. None for engine-injected.
    pub mcp_id: Option<String>,
    /// True iff this tool came from `tools::build_injected_tools`.
    pub injected: bool,
}

pub enum ToolResultPayload {
    Ok { result_json: String },
    Error { message: String },
    /// Sent automatically by the bridge for tools the engine has not yet
    /// implemented. M5 will replace this auto-reply with real dispatch.
    Unsupported,
}
```

`OutcomeReported` and `HumanInputRequested` are deliberately **not** sub-variants of `ToolCall` — they're first-class events because M5 needs to route them to entirely different state-machine paths than generic tool calls.

### 4.6 `Sandbox` trait

```rust
pub enum SandboxDecision {
    Allow,
    Deny { reason: String },
    /// The caller (engine, M5) must request elevation from the user. Until
    /// elevated, the tool result is `ToolResultPayload::Error("sandbox: requires
    /// elevation")`.
    Elevate { capability: String },
}

pub trait Sandbox: Send + Sync {
    /// Called once per tool at session-open time to decide whether to expose it.
    /// Returning `Deny` removes the tool from the agent's visible tool list.
    /// Returning `Allow` or `Elevate` keeps it visible (Elevate is decided
    /// per-call when the agent actually invokes it).
    fn visibility(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision;

    /// Called once per actual tool invocation. Bridge attaches the decision
    /// to BridgeEvent::ToolCall so M5 can route appropriately.
    fn allows_tool(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision;

    /// Required for `SessionConfig: Clone`. Implementations return a boxed clone
    /// of themselves. (`dyn Trait` cannot derive Clone directly.)
    fn boxed_clone(&self) -> Box<dyn Sandbox>;
}

#[derive(Clone, Debug)]
pub struct AlwaysAllowSandbox;
impl Sandbox for AlwaysAllowSandbox { /* both methods → Allow */ }

#[derive(Clone, Debug)]
pub struct DenyListSandbox {
    pub denied_tools: HashSet<String>,
    pub denied_mcp_ids: HashSet<String>,
}
impl Sandbox for DenyListSandbox { /* see §6.4 */ }
```

`Sandbox::visibility` and `Sandbox::allows_tool` are deliberately **separate** functions even though `DenyListSandbox` returns the same answer for both. M4 will introduce a `PathSandbox` whose visibility (always allow `read_text_file` if the worktree is a real path) differs from per-call decisions (deny if the requested path escapes the worktree). M3 ships the surface so M4 just adds an impl.

### 4.7 Errors

```rust
pub enum BridgeError {
    /// Worker thread panicked or exited unexpectedly. Bridge is dead; drop & respawn.
    WorkerDead,
    /// Command channel send failed (worker likely already shutting down).
    CommandSendFailed(String),
    /// oneshot reply dropped before sending (worker died mid-command).
    ReplyDropped,
}

pub enum OpenSessionError {
    /// Subprocess spawn failed (binary missing, exec permission denied, etc.).
    AgentSpawnFailed { kind: String, source: std::io::Error },
    /// ACP handshake failed (incompatible version, malformed initial frame).
    HandshakeFailed { reason: String },
    /// Session would have started but the empty `declared_outcomes` makes
    /// `report_stage_outcome` unconstructible.
    NoDeclaredOutcomes,
    /// Tool list is invalid (duplicate names, schema validation failed).
    InvalidToolDefs(String),
    Bridge(#[source] BridgeError),
}

pub enum SendMessageError {
    SessionNotFound { session: SessionId },
    SessionEnded { session: SessionId, reason: SessionEndReason },
    Bridge(#[source] BridgeError),
}

pub enum CloseSessionError {
    SessionNotFound { session: SessionId },
    /// Graceful shutdown didn't complete in `close_grace_ms` (default 5000); the
    /// child was killed and the session is gone, but reported here so callers
    /// know the closure was not clean.
    GracefulTimedOut { session: SessionId, killed: bool },
    Bridge(#[source] BridgeError),
}

pub enum AcpError {
    /// Underlying ACP SDK returned an error.
    Protocol(#[source] agent_client_protocol::Error),
    /// I/O error reading/writing the agent subprocess streams.
    Io(#[source] std::io::Error),
    /// Subprocess exited mid-handshake.
    AgentExited { exit_code: Option<i32> },
}
```

`BridgeError` and the legacy `SurgeError` deliberately do not have `From` between them — they belong to different domains.

## 5. Internals

### 5.1 Bridge thread bootstrap

```rust
impl AcpBridge {
    pub fn spawn(cmd_capacity: usize, event_capacity: usize) -> Result<Self, BridgeError> {
        let (cmd_tx, cmd_rx) = mpsc::channel(cmd_capacity);
        let (event_tx, _) = broadcast::channel(event_capacity);
        let event_tx_clone = event_tx.clone();

        let thread = std::thread::Builder::new()
            .name("surge-acp-bridge".to_string())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| BridgeError::WorkerDead)?;
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, worker::bridge_loop(cmd_rx, event_tx_clone))
            })
            .map_err(|_| BridgeError::WorkerDead)?;

        Ok(Self {
            cmd_tx,
            event_tx,
            worker: Some(thread),
        })
    }
}
```

Sane bounded mpsc capacity (default 64). If the engine produces commands faster than the bridge can drain them — that's a bug in the engine; backpressure surfaces as `send().await` blocking.

### 5.2 `bridge_loop`

```rust
pub async fn bridge_loop(
    mut cmd_rx: mpsc::Receiver<BridgeCommand>,
    event_tx: broadcast::Sender<BridgeEvent>,
) -> Result<(), BridgeError> {
    let sessions: Rc<RefCell<HashMap<SessionId, AcpSession>>> = Rc::default();

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            BridgeCommand::OpenSession { config, reply } => {
                let result = open_session_impl(&sessions, &event_tx, config).await;
                let _ = reply.send(result);
            }
            BridgeCommand::SendMessage { session, content, reply } => {
                let result = send_message_impl(&sessions, session, content).await;
                let _ = reply.send(result);
            }
            BridgeCommand::SessionState { session, reply } => {
                let result = session_state_impl(&sessions, session);
                let _ = reply.send(result);
            }
            BridgeCommand::CloseSession { session, reply } => {
                let result = close_session_impl(&sessions, &event_tx, session).await;
                let _ = reply.send(result);
            }
            BridgeCommand::Shutdown { reply } => {
                close_all_sessions(&sessions, &event_tx, SessionEndReason::ForcedClose).await;
                let _ = reply.send(());
                return Ok(());
            }
        }
    }
    Ok(())
}
```

Sessions live in `Rc<RefCell<HashMap>>` because everything runs on the same thread (the LocalSet) — `Rc/RefCell` is the right primitive, no mutex contention.

Inside `open_session_impl`, after the ACP handshake completes, the bridge spawns two background `spawn_local` tasks per session:

1. **Session observer.** Reads `SessionUpdate` notifications from the ACP connection, classifies them (agent message, tool call, tool result, token usage), emits the appropriate `BridgeEvent`.
2. **Subprocess waiter.** `child.wait().await` — when the child exits, emits `SessionEnded { reason: AgentCrashed { exit_code, stderr_tail } }` and removes the session from the map.

Both tasks share an `Rc<SessionStateInner>` and gracefully terminate when the session is closed.

### 5.3 Tool injection mechanism

ACP 0.10.2 lets the client declare tools to the agent during the `InitializeRequest` handshake (under `client_capabilities.tools`). The bridge:

1. Computes the visible tool list via `Sandbox::visibility` over `config.tools + injected_tools`.
2. Adds `report_stage_outcome` (always, with dynamic `enum` from `declared_outcomes`).
3. Adds `request_human_input` iff `config.allows_escalation`.
4. Validates no duplicate names; returns `OpenSessionError::InvalidToolDefs` if any.
5. Sends the resulting list as part of `InitializeRequest`.

When the agent invokes a tool, the bridge receives `SessionUpdate::ToolCallPending` (or equivalent for the SDK version we're on — exact discriminant verified in plan phase). Routing:

- If `tool == "report_stage_outcome"`: parse args, emit `BridgeEvent::OutcomeReported`, auto-reply with `ToolResultPayload::Ok` so the agent knows the stage is over.
- If `tool == "request_human_input"`: parse args, emit `BridgeEvent::HumanInputRequested`, **do not** auto-reply — M5 will provide the answer via a future `AcpBridge::reply_to_tool(call_id, payload)` method. M3 stub: auto-reply with `Unsupported` after a hard timeout (10s) to prevent dangling sessions in M3 integration tests.
- Otherwise: emit `BridgeEvent::ToolCall { sandbox_decision, .. }`. In M3, the bridge auto-replies with `Unsupported` immediately (M5 will replace this with real dispatch).

The auto-reply policy lets M3 ship a complete bridge that the mock agent can drive end-to-end without M5 being implemented. M5 will provide a `ToolDispatcher` callback installed at `AcpBridge::spawn()` time:

```rust
// Reserved for M5 (not part of M3 API):
pub type ToolDispatcher =
    Arc<dyn Fn(SessionId, ToolCallMeta, String) -> BoxFuture<'static, ToolResultPayload> + Send + Sync>;
```

M3 ships the architectural seam (the auto-reply path is a single match arm to replace) without committing to the full callback shape.

### 5.4 `report_stage_outcome` schema construction

```rust
pub fn build_report_stage_outcome_tool(declared_outcomes: &[OutcomeKey]) -> ToolDef {
    assert!(!declared_outcomes.is_empty(), "M3 contract: caller must check");
    let outcomes_json: Vec<serde_json::Value> = declared_outcomes
        .iter()
        .map(|k| serde_json::Value::String(k.as_str().to_string()))
        .collect();
    ToolDef {
        name: "report_stage_outcome".into(),
        description: "Report your stage's outcome. Call this exactly once at the end."
            .into(),
        category: ToolCategory::Injected,
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["outcome", "summary"],
            "properties": {
                "outcome": {
                    "type": "string",
                    "enum": outcomes_json,
                    "description": "Which declared outcome best describes your result"
                },
                "summary": {
                    "type": "string",
                    "description": "1-3 sentences explaining what you did and why this outcome"
                },
                "artifacts_produced": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of file paths you created or modified"
                }
            }
        }),
    }
}
```

The dynamic `enum` is the entire reason the engine has to construct this tool fresh per session. Two stages with different `OutcomeDecl` lists must see different enums.

### 5.5 Sandbox filtering at session open

```rust
fn filter_visible_tools(
    tools: Vec<ToolDef>,
    sandbox: &dyn Sandbox,
) -> (Vec<ToolDef>, Vec<String>) {
    let mut visible = Vec::with_capacity(tools.len());
    let mut hidden_names = Vec::new();
    for t in tools {
        let mcp_id = t.category.mcp_id();
        match sandbox.visibility(&t.name, mcp_id) {
            SandboxDecision::Allow | SandboxDecision::Elevate { .. } => visible.push(t),
            SandboxDecision::Deny { .. } => hidden_names.push(t.name.clone()),
        }
    }
    (visible, hidden_names)
}
```

Tools with `Elevate` visibility stay in the list — the agent sees them, but per-call invocations get `Elevate` in `BridgeEvent::ToolCall::sandbox_decision` and M5 routes to elevation flow. Tools with `Deny` visibility are removed; the agent literally doesn't know they exist.

### 5.6 Crash detection

For each open session, the bridge spawns:

```rust
let waiter = tokio::task::spawn_local({
    let sessions = sessions.clone();
    let event_tx = event_tx.clone();
    let session_id = session_id.clone();
    async move {
        let exit_status = child.wait().await;
        let stderr_tail = read_stderr_tail(&mut stderr_pipe, 2048).await;
        let reason = match exit_status {
            Ok(s) if s.success() => SessionEndReason::Normal,
            Ok(s) => SessionEndReason::AgentCrashed {
                exit_code: s.code(),
                stderr_tail,
            },
            Err(_) => SessionEndReason::AgentCrashed { exit_code: None, stderr_tail },
        };
        let _ = event_tx.send(BridgeEvent::SessionEnded { session: session_id.clone(), reason });
        sessions.borrow_mut().remove(&session_id);
    }
});
```

The waiter task is awaited as part of the session's `Drop` cleanup. `read_stderr_tail` is a small bounded read — keeps memory bounded if the agent dumps a huge stack trace.

**Two stderr buffers, two purposes.** The 2 KiB tail captured here at process exit is intentionally smaller than the 8 KiB running drain buffer described in §11.4. They serve different roles:

- **8 KiB ring (`stderr_drainer` task, runs throughout the session)** — exists to relieve pipe-buffer pressure (~64 KiB on Linux, smaller on Windows) so the agent never blocks on `write(stderr)`. Drained continuously, flushed to `tracing::warn!` for post-mortem inspection in logs. Ring size is generous because the consumer is a log file, not a network frame.
- **2 KiB tail (`read_stderr_tail`, runs once at exit)** — exists to populate `SessionEndReason::AgentCrashed::stderr_tail`, which is broadcast to *every* subscriber and ultimately persisted as part of `EventPayload::SessionClosed` in M5. Kept compact so the event payload stays small. Reads the most recent 2 KiB at exit, which may be a subset of (or overlap with) what the drainer task has already logged.

**Unified `tail_storage` shared between drainer and waiter.** The drainer feeds `tail_storage` (an `Rc<RefCell<Vec<u8>>>`); the waiter reads it via `read_stderr_tail` at exit time. The drainer enforces the cap at every write — `tail_storage.len()` is bounded by `STDERR_TAIL_CAP` (2 KiB), so the `SessionEnded` payload size guarantee holds without a separate exit-time pipe read.

The original §5.6 design called for two independent buffers (drainer's own ring + a separate exit-time pipe read). The unified-buffer approach replaces it for these reasons:
- Avoids racing the drainer's last reads when the waiter does its own pipe read at exit time.
- Simpler ownership: only the drainer owns the `ChildStderr` handle.
- Same payload size guarantee — the drainer's cap is the load-bearing invariant.

`STDERR_RING_CAP` (8 KiB) is retained as a defensive belt-and-suspenders bound; in the unified-buffer design it is logically unreachable (the TAIL_CAP check fires first), but kept for future-proofing if the buffer caps ever decouple.

Acceptance: integration test `bridge_crash_detection.rs` SIGKILLs the mock agent, expects `SessionEnded` event within 2 s. Achievable on all three OSes because tokio's `Child::wait` polls process exit at runtime tick granularity (~ms).

### 5.7 Token usage extraction

ACP 0.10.2 with `unstable_session_usage` exposes per-`SessionUpdate` usage metadata. The exact field path is verified in the plan phase (likely `SessionUpdate::AgentMessage { usage: Option<UsageBlock>, .. }` or similar). The bridge:

1. On every `SessionUpdate` carrying usage data: emit `BridgeEvent::TokenUsage { session, prompt_tokens, output_tokens, cache_hits, model }`.
2. The values are **cumulative** (not deltas) — that's how the SDK reports them. M5 stores the latest per session in the materialized view (`cost_summary`).

If the underlying agent doesn't report usage (some agents don't), the bridge emits no `TokenUsage` events for that session. Tests cover both code paths via the mock agent, which can be configured to emit usage blocks (`MOCK_ACP_USAGE=on`).

**Ordering guarantee.** The bridge guarantees that every `BridgeEvent::TokenUsage` for a given `SessionId` is emitted **before** the `BridgeEvent::SessionEnded` for that session. The session close path explicitly flushes the last pending usage block (if any) before emitting the terminator. M5 can rely on this when finalizing `cost_summary` — there is no race where a stray `TokenUsage` arrives after the engine has already marked the session terminal in its materialized view, so M5 can free per-session aggregation state on `SessionEnded` without buffering "late" usage.

Verified by integration test `bridge_token_tracking` (§9.2): after the mock agent emits N usage blocks then exits, the subscriber observes N `TokenUsage` events strictly preceding `SessionEnded::Normal`.

### 5.8 Secrets redaction

`crate::secrets::redact_secrets` already does regex-based redaction of common API key formats (AWS, OpenAI, Anthropic, GitHub PAT, generic `Bearer` tokens). M3 reuses it — every `BridgeEvent::ToolCall` runs `args_redacted_json = redact_secrets(&args_json)` before emission.

The legacy `SurgeClient` already redacts arguments in `SurgeEvent::ToolCalled`; the new bridge does the same for `BridgeEvent::ToolCall`. Future work (out of scope for M3): expand the regex set per RFC-0006 §secret-handling.

### 5.9 `BridgeClient` state and trait impl shape

`BridgeClient` is the bridge-side ACP `Client` trait impl (see §3 layout, `bridge/client.rs`, ~250 LOC). Each open session gets exactly one `BridgeClient` instance, owned by the session's ACP connection. ACP invokes its 11 methods (`request_permission`, `write_text_file`, `read_text_file`, `create_terminal`, `terminal_output`, `wait_for_terminal_exit`, `kill_terminal`, `release_terminal`, `session_notification`, `ext_request`, `ext_notification`) asynchronously from the connection task. The struct shape is fixed at compile time; new state for M5 (e.g. the `ToolDispatcher` callback) is added as `Option<...>` fields with `None` default to avoid breaking M3 tests.

**State:**

```rust
pub(crate) struct BridgeClient {
    /// Stable id; labels every emitted event for this session.
    session_id: SessionId,

    /// Cloned from the bridge worker; events fan out to all subscribers.
    event_tx: broadcast::Sender<BridgeEvent>,

    /// Per-session mutable state (open tool calls, ACP-side session string,
    /// last-seen token usage). `Rc<RefCell>` because everything runs on the
    /// LocalSet thread — single-threaded, no atomics needed.
    state: Rc<RefCell<SessionStateInner>>,

    /// Boxed because the trait is dyn-typed in `SessionConfig`. Cloned per-session
    /// at open time via `Sandbox::boxed_clone`.
    sandbox: Box<dyn Sandbox>,

    /// Process-wide redactor for tool args and `write_text_file` payloads.
    /// `Arc` lets one regex set serve every session.
    secrets: Arc<SecretsRedactor>,

    /// Engine-supplied bindings; passed through to `BridgeEvent::SessionEstablished`
    /// and ignored by ACP itself.
    bindings: BTreeMap<String, String>,

    /// Worktree root; used by `crate::shared::path_guard` helpers when validating
    /// `write_text_file` / `read_text_file` paths. Same role as in legacy `SurgeClient`.
    worktree_root: PathBuf,

    /// Terminal manager; reused from legacy via `crate::shared::content_block`.
    /// Per-session because terminal lifetime ties to the session.
    terminals: Arc<Mutex<crate::terminal::Terminals>>,
}
```

**Sketch impl** (illustrative — full impl is M3 work, but the architectural seams are fixed here):

```rust
impl Client for BridgeClient {
    async fn request_permission(
        &self,
        req: RequestPermissionRequest,
    ) -> AcpResult<RequestPermissionResponse> {
        // ACP request_permission is the per-call permission check (e.g. "agent
        // wants to run shell command X — allow?"). The Sandbox decides; M5 will
        // route Elevate to a UI/Telegram flow via BridgeEvent::ToolCall.
        let mcp_id = extract_mcp_id_from_request(&req);
        let tool_name = extract_tool_name_from_request(&req);
        let decision = self.sandbox.allows_tool(&tool_name, mcp_id.as_deref());
        match decision {
            SandboxDecision::Allow => Ok(allow_response(&req)),
            SandboxDecision::Deny { reason } => Ok(deny_response(&req, &reason)),
            SandboxDecision::Elevate { capability } => {
                // M3 stub: deny with hint. M5 will replace with an async wait
                // on an engine-supplied callback (see ToolDispatcher seam in §5.3).
                Ok(deny_response(&req, &format!("requires elevation: {capability}")))
            }
        }
    }

    async fn write_text_file(&self, req: WriteTextFileRequest) -> AcpResult<WriteTextFileResponse> {
        crate::shared::path_guard::ensure_in_worktree(&self.worktree_root, &req.path)?;
        // ... actual write happens. No BridgeEvent emitted — file IO is observable
        // through tool call results, not through Client trait events. Errors during
        // write are returned to the agent as the AcpResult.
        do_write(&req).await
    }

    // ... 9 more methods, each:
    //   1. Validate via crate::shared::path_guard or self.sandbox
    //   2. Do the work
    //   3. Emit BridgeEvent only when relevant — most are silent because ACP
    //      carries its own observability for IO operations.
}
```

The above pattern (validate → do → emit-only-when-relevant) keeps `BridgeClient` thin. Heavy lifting (event correlation, tool dispatch, sandbox elevation routing) lives in the worker task and in M5; `BridgeClient` is just the ACP-facing adapter.

## 6. Sandbox trait — interim M3 surface

§4.6 defines the trait. This section pins down the **two impls** shipped in M3 and how they behave.

### 6.1 `AlwaysAllowSandbox`

```rust
impl Sandbox for AlwaysAllowSandbox {
    fn visibility(&self, _: &str, _: Option<&str>) -> SandboxDecision { SandboxDecision::Allow }
    fn allows_tool(&self, _: &str, _: Option<&str>) -> SandboxDecision { SandboxDecision::Allow }
    fn boxed_clone(&self) -> Box<dyn Sandbox> { Box::new(self.clone()) }
}
```

Used by mock-agent integration tests, by `vibe doctor` smoke tests, and by the local development default (`SessionConfig::dev()` factory in M5).

### 6.2 `DenyListSandbox`

```rust
impl Sandbox for DenyListSandbox {
    fn visibility(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision {
        if self.denied_tools.contains(tool) {
            return SandboxDecision::Deny { reason: format!("tool {tool} is denied") };
        }
        if let Some(id) = mcp_id {
            if self.denied_mcp_ids.contains(id) {
                return SandboxDecision::Deny { reason: format!("mcp server {id} is denied") };
            }
        }
        SandboxDecision::Allow
    }
    fn allows_tool(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision {
        // Per RFC-0006: same denylist applies at call time. visibility==Deny implies
        // tool is filtered out, so allows_tool would never see it for denied tools.
        // Symmetric impl for clarity; tested for parity in property tests.
        self.visibility(tool, mcp_id)
    }
    fn boxed_clone(&self) -> Box<dyn Sandbox> { Box::new(self.clone()) }
}
```

Used by the `bridge_sandbox_filtering.rs` integration test. Sufficient to cover RFC-0006 §Tier-1 enforcement; tier 2 (path) and tier 3 (OS) come in M4.

### 6.3 Future `Sandbox` impls (M4 scope, listed for completeness)

- `WorkspaceWriteSandbox` — read-only outside worktree, write inside; visibility-allow but call-time path checks.
- `LandlockSandbox` (Linux) / `SandboxExecSandbox` (macOS) / `JobObjectSandbox` (Windows) — wrap above with OS enforcement.
- `NetworkAllowlistSandbox` — domain allowlist for outbound HTTPS.

M3 ships none of these. The trait surface defined in §4.6 is sufficient for them to land later without breaking M3 callers.

## 7. Mock ACP agent

`crates/surge-acp/src/bin/mock_acp_agent.rs` — a small binary (target ~300 LOC) that speaks ACP over stdio and exhibits deterministic behavior controlled by env vars and CLI args.

### 7.1 Behaviors

| Env / arg                           | Behavior                                                                     |
|-------------------------------------|------------------------------------------------------------------------------|
| `--scenario echo`                   | Echo the user message back as `AgentMessage`.                                |
| `--scenario report_done`            | After receiving any user message, call `report_stage_outcome { outcome: "done", summary: "mock" }`. |
| `--scenario report_outcome=<key>`   | Same but with a configurable outcome key (validated against the declared enum). |
| `--scenario crash_after=<n>`        | Process N tool calls, then `std::process::exit(137)` to simulate SIGKILL.    |
| `--scenario human_input`            | Call `request_human_input { question: "what now?" }` and wait for the bridge reply. |
| `--scenario long_streaming`         | Emit 20 `AgentMessage` chunks with 50 ms delay between each.                 |
| `MOCK_ACP_USAGE=on`                 | Include `unstable_session_usage` token usage metadata in agent messages.     |
| `MOCK_ACP_HANDSHAKE_FAIL=1`         | Exit with code 1 before writing the ACP initialize response. The bridge sees the subprocess exit during handshake; depending on which side of the race wins, this surfaces as `OpenSessionError::HandshakeFailed` or `OpenSessionError::AgentSpawnFailed`. |
| `MOCK_ACP_LOG=stderr`               | Print verbose stderr for `read_stderr_tail` testing.                         |

### 7.2 Implementation approach

Use the same `agent-client-protocol` crate from the **agent side** — the SDK provides `Agent` trait and `AgentSideConnection` constructs symmetric to `Client`/`ClientSideConnection`. The mock impls `Agent` with hard-coded responses driven by the scenario flag.

This keeps the mock honest: it speaks the real ACP, so any ACP-spec change shows up in the mock the same way it shows up in real agents. Saves us from hand-rolling a JSON-RPC fixture that would drift from reality.

### 7.3 Discoverability

Bridge tests resolve the mock binary via `CARGO_BIN_EXE_mock_acp_agent`. The `AgentKind::Mock { args }` variant short-circuits the binary lookup — bridge knows the mock is always at the env path.

## 8. Workspace dependency additions

The bridge reuses what's already in workspace deps. The few new ones:

In root `Cargo.toml [workspace.dependencies]`:

```toml
# Already present, listed for clarity:
agent-client-protocol = { version = "0.10.2", features = ["unstable_session_usage"] }
tokio = { version = "1", features = ["macros", "rt", "rt-multi-thread", "sync", "time", "io-util", "process", "signal"] }
tracing = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
ulid = { version = "1", features = ["serde"] }
regex = "1"

# New for M3:
async-trait = "0.1"           # for the `Sandbox` trait (uses async fn? — see open question 14.2)
```

In `crates/surge-acp/Cargo.toml [dependencies]`:

```toml
# All workspace inherits — no new deps that aren't already present in the legacy modules.
agent-client-protocol = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt", "sync", "time", "io-util", "process", "signal"] }
tracing = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
ulid = { workspace = true }
regex = { workspace = true }
surge-core = { workspace = true }
```

In `crates/surge-acp/Cargo.toml [[bin]]`:

```toml
[[bin]]
name = "mock_acp_agent"
path = "src/bin/mock_acp_agent.rs"
required-features = []
```

In `crates/surge-acp/Cargo.toml [dev-dependencies]`:

```toml
tokio = { workspace = true, features = ["test-util", "macros", "rt"] }
tempfile = { workspace = true }
```

No new top-level workspace deps needed if `async-trait` turns out unnecessary (open question §14.2).

## 9. Testing strategy

Tests live in `crates/surge-acp/tests/` (integration) for full bridge round-trips, with unit tests in each new `bridge::*` module.

### 9.1 Unit tests (per module)

- `bridge::tools` — `build_report_stage_outcome_tool` with various outcome lists; rejects empty list; produces valid JSON Schema.
- `bridge::sandbox` — `AlwaysAllowSandbox` and `DenyListSandbox` decision tables; `boxed_clone` round-trip preserves behavior.
- `bridge::tokens` — extracts cumulative usage; missing fields → no event; malformed payload → bridge logs warn but doesn't crash.
- `bridge::session` — `SessionConfig` validation rejects empty `declared_outcomes`, duplicate tool names, oversized `bindings`.
- `bridge::event` — `BridgeEvent` round-trip via serde_json (for future replay/persistence even though M3 doesn't persist).

### 9.2 Integration tests (mock agent driven)

| Test                              | Intent                                                                   |
|-----------------------------------|--------------------------------------------------------------------------|
| `bridge_session_lifecycle`        | open → text message → mock echoes → close. Verify `SessionEstablished` then `AgentMessage` then `SessionEnded::Normal`. |
| `bridge_tool_injection`           | open with `declared_outcomes = ["done", "blocked"]`. Mock calls `report_stage_outcome { outcome: "done" }`. Verify `BridgeEvent::OutcomeReported` (not generic ToolCall). |
| `bridge_dynamic_outcome_enum`     | Two parallel sessions with different outcome lists. Each agent sees its own enum; bridge accepts each correctly. |
| `bridge_request_human_input`      | Mock calls `request_human_input`. Verify `HumanInputRequested` event and that no auto-reply happens before the M3 timeout. |
| `bridge_sandbox_filtering`        | Open with `DenyListSandbox { denied_tools: {"shell_exec"} }` and a `tools` list including `shell_exec`. Verify `tools_visible` in `SessionEstablished` does not include it. |
| `bridge_crash_detection`          | Open mock with `--scenario crash_after=2`. Drive 2 tool calls. Expect `SessionEnded { AgentCrashed }` ≤2s, `exit_code = Some(137)`, `stderr_tail` contains the mock's last log line. |
| `bridge_concurrent_sessions`      | Open 5 sessions in parallel, drive each independently. Verify no event interleaving lost, no deadlock, all close cleanly. |
| `bridge_token_tracking`           | Open with `MOCK_ACP_USAGE=on`. Drive 3 messages. Verify `TokenUsage` events with monotonically increasing cumulative counts. |
| `bridge_streaming`                | `--scenario long_streaming`. Verify 20 `AgentMessage` events arrive in order, each within ~70 ms of the previous (50 ms delay + IPC overhead). |
| `bridge_handshake_failure`        | `MOCK_ACP_HANDSHAKE_FAIL=1`. Verify `OpenSessionError::HandshakeFailed`. |
| `bridge_close_timeout`            | Mock with frozen scenario (sleeps forever). `close_session` returns `GracefulTimedOut { killed: true }`; subsequent `SessionEnded::Timeout` event observed. |
| `bridge_shutdown_with_open`       | Open 2 sessions, call `AcpBridge::shutdown()`. Verify both emit `SessionEnded::ForcedClose`; bridge thread exits cleanly. |
| `bridge_worker_panic`             | Inject a panic into the worker thread (via a test-only `BridgeCommand::TestPanic` variant gated by `#[cfg(test)]`). Verify subsequent `cmd_tx.send()` returns `BridgeError::CommandSendFailed`; subscribers observe no further events; the thread `JoinHandle` reports the panic when consumed. Covers the `WorkerDead` recovery path that would otherwise be untested. |

**Default sandbox in non-sandbox tests.** All tests except `bridge_sandbox_filtering` use `AlwaysAllowSandbox`. Sandbox-specific tests construct `DenyListSandbox` with the denylist documented inline in the test setup. Mixing sandboxes across tests is intentional — keeps the hot-path tests (lifecycle, streaming, crash) focused on bridge mechanics rather than sandbox policy.

### 9.3 Property tests (`proptest`)

| Test                                | Property                                                               |
|-------------------------------------|------------------------------------------------------------------------|
| `tool_def_validation_total`         | For any random `Vec<ToolDef>`, validation either succeeds or returns a specific named error variant; never panics. |
| `sandbox_visibility_idempotent`     | For `AlwaysAllow` and `DenyList`, `visibility(t, m)` is consistent with `allows_tool(t, m)` modulo the documented future divergence (per §4.6). |
| `outcome_enum_serializable`         | For any `Vec<OutcomeKey>` of size 1..32, the JSON Schema produced by `build_report_stage_outcome_tool` is valid JSON Schema and round-trips. |

### 9.4 Snapshot tests (`insta`)

- One handcrafted `BridgeEvent` sequence (open → 3 messages → tool call → outcome → close) → snapshot the Vec<BridgeEvent> via `serde_json`. Catches accidental field reorderings or rename regressions.
- One snapshot of the JSON Schema produced by `build_report_stage_outcome_tool(&[ok("done"), ok("blocked"), ok("escalate")])`.

### 9.5 Mock-agent self-test

A unit test inside the mock binary (gated by `#[cfg(test)]`) verifies the mock's own scenario parser handles every flag without crashing. Cheap insurance against the mock breaking silently.

### 9.6 No criterion benchmarks in M3

Bridge perf budgets are not yet binding. Add benchmarks if/when M5 engine surfaces a real bottleneck (likely candidates: `redact_secrets` on huge tool args, JSON roundtrip on 100k-token messages).

## 10. Acceptance criteria

The milestone is complete when **all** of the following pass:

1. `cargo build -p surge-acp` clean on Linux, macOS, Windows.
2. `cargo test -p surge-acp` passes — all unit, integration, property, and snapshot tests.
3. `cargo clippy -p surge-acp --all-targets -- -D warnings` clean for the new `bridge::*` and `shared::*` modules. Legacy modules (`client`, `connection`, `pool`, `discovery`, `display`, `health`, `process_tracker`, `registry`, `router`, `secrets`, `terminal`, `transport`) remain on the workspace's permissive clippy set per the M2 precedent (see M2 §11 acceptance #17).
4. `cargo build --workspace` succeeds — `surge-orchestrator`, `surge-cli`, `surge-ui`, `surge-spec` compile unchanged (pure addition guarantee).
5. `cargo build --bin mock_acp_agent -p surge-acp` produces a working ACP-speaking mock binary on all three OSes.
6. All 13 integration tests in §9.2 pass deterministically.
7. `bridge_crash_detection` consistently surfaces `SessionEnded` within 2 s of mock SIGKILL on all three OSes (per docs/revision/04-acp-integration.md §Acceptance #5).
8. `bridge_concurrent_sessions` runs 5 sessions in parallel without deadlock or event loss (per docs/revision/04-acp-integration.md §Acceptance #6).
9. `bridge_dynamic_outcome_enum` proves two sessions can use distinct outcome enums concurrently (per docs/revision/04-acp-integration.md §Acceptance #3).
10. `bridge_sandbox_filtering` proves disallowed tools never appear in `BridgeEvent::SessionEstablished::tools_visible` (per docs/revision/04-acp-integration.md §Acceptance #4).
11. `bridge_streaming` proves real-time streaming (events visible to subscribers within agent emission cadence) (per docs/revision/04-acp-integration.md §Acceptance #7).
12. `bridge_token_tracking` proves cumulative token usage is reported and monotonic (per docs/revision/04-acp-integration.md §Acceptance #8).
13. All public API in `bridge::*` documented with `///`; `cargo doc -p surge-acp --no-deps --document-private-items` produces no warnings on the new modules.
14. The `Sandbox` trait surface is stable enough that M4's planned impls need no breaking changes. Verified by shipping a `WorkspaceWriteSandbox` stub in `tests/bridge_sandbox_m4_stub.rs`. The stub:
    - Implements `Sandbox` against §4.6 with **no** modifications to the trait.
    - Returns `visibility = Allow` for `read_text_file`, `write_text_file`, and `list_directory`.
    - Returns `allows_tool = Deny { reason: "path escapes worktree" }` for `write_text_file` when the args' path resolves outside the supplied worktree root, otherwise `Allow`.
    - Includes one smoke test asserting the divergence between `visibility` and `allows_tool` for a path that escapes the worktree — this is the very case that motivates the two-method split in §4.6, so the M3 acceptance gate exercises the asymmetric path explicitly.
    
    This stub is **not** a real M4 impl: no OS enforcement, no canonical-path resolution against symlinks, no read/write distinction beyond the example. Its purpose is to lock the trait surface so M4 can land its real impls additively.
    
    `WorkspaceWriteSandbox` is chosen over the alternative stubs because it best exercises the visibility/allows-tool split:
    - `LandlockSandbox` (Linux-only) — distracts from cross-platform CI; trait surface check doesn't need OS specifics.
    - `NetworkAllowlistSandbox` (cross-platform) — doesn't naturally exercise the visibility-vs-call divergence (network tools are usually all-or-nothing visible).
    - `WorkspaceWriteSandbox` ✓ — visibility is `Allow` for the IO tools but `allows_tool` is path-dependent, exactly the asymmetry §4.6 is built for.
15. End-to-end CLI integration with a real Claude Code (per docs/revision/04-acp-integration.md §Acceptance #9) is **NOT** part of M3 acceptance — that test ships in M5/M8 once the engine and a CLI driver exist. M3 ships the bridge that M5 and M8 will use; verifying it against a real agent is their job.

## 11. Risks & known unknowns

### 11.1 ACP 0.10.2 tool injection surface

Architecture doc (docs/revision/04-acp-integration.md §Tool injection) describes "client-side tools" as a first-class ACP concept. The exact API in `agent-client-protocol = 0.10.2` needs verification:

- Are tools declared per-session via `InitializeRequest::client_capabilities.tools`, or registered via a separate `tools/list` callback the agent can poll?
- Does the SDK already provide a typed `ToolDefinition` shape, or do we wire JSON manually?
- Does `unstable_session_usage` deliver token counts on every chunk, only on terminal frames, or both?

These answers shape parts of `tools.rs` and `tokens.rs`. They're verified in the writing-plans phase by reading `agent-client-protocol`'s rustdoc and source. If the SDK's surface is materially different, the spec is updated before plan execution; the test interfaces (mock scenarios, BridgeEvent variants) likely don't change.

### 11.2 `!Send` in `agent-client-protocol`

The legacy code already lives with this. The bridge inherits the same constraint and deals with it via the dedicated `LocalSet` thread. Risk is low.

### 11.3 Mock binary discoverability across `cargo test` invocations

`CARGO_BIN_EXE_mock_acp_agent` is set by Cargo when the integration test is built — but only if Cargo built the binary first. Standard pattern: the test crate declares `required-features` so `cargo test -p surge-acp` builds the binary as part of test compilation. Risk: forgotten flag → test fails with "binary not found." Mitigation: add a build-time `build.rs` check in the plan phase that asserts the env var is set when tests run.

### 11.4 ACP subprocess stderr blocking

If the mock or real agent dumps unbounded stderr and the bridge doesn't drain it, the OS pipe buffer fills (~64 KB on Linux) and the agent process blocks on `write(stderr)`. Bridge **must** continuously drain stderr into a bounded ring buffer (`tokio::io::AsyncReadExt::read` into a `Vec` capped at 8 KiB, dropping older bytes). The `read_stderr_tail` in §5.6 does the final read; throughout the session, a separate `spawn_local` task drains stderr.

### 11.5 Shutdown ordering

`AcpBridge::shutdown()` must not deadlock when sessions hold references to objects the bridge owns. The pattern in §5.2 (close_all_sessions before reply) ensures all session futures complete before the worker exits. Tested via `bridge_shutdown_with_open`.

### 11.6 Cross-process bridge sharing and per-process bridge count

Two surge processes (e.g., daemon + CLI) cannot share a bridge — each has its own. They can talk to the same agent only by spawning their own subprocess, which is the intended model. Documented in `AcpBridge` rustdoc; no enforcement needed (it's structurally impossible anyway).

Within a single process, the typical pattern is **one** `AcpBridge` shared by all sessions (engine owns it; CLI talks to engine over a separate channel rather than embedding a second bridge). Spawning multiple `AcpBridge` instances in one process is supported but **not recommended**: there is no resource sharing between bridges, so two bridges means two LocalSet threads, two sets of broadcast subscribers, two mpsc command queues — wasteful with no upside. Documented in `AcpBridge::spawn` rustdoc as a soft-discouragement, not a hard error (callers in tests legitimately want multiple bridges to verify isolation).

### 11.7 Token usage and `unstable_session_usage` future-proofing

The feature is named `unstable_*` for a reason — the API may shift in 0.11 or 0.12. M3 isolates the extraction to `bridge::tokens` so a future SDK upgrade touches one file. We do **not** expose the SDK type in `BridgeEvent::TokenUsage` — that field is plain `u32`/`String`, decoupling consumers from SDK churn.

### 11.8 Lagged broadcast subscribers and event loss

`BridgeEvent` is fan-out via `tokio::sync::broadcast` (capacity default 1024). When a subscriber lags more than the capacity, it gets `RecvError::Lagged(skipped_count)` on its next `recv()` and resumes from the newest event — older events are **silently dropped** for that subscriber.

For the M5 engine, which subscribes specifically to persist events into the durable run log via `RunWriter` (M2), a dropped event is a **lost `RunEvent`** and corrupts the materialized view (`cost_summary`, `stage_executions`, `pending_approvals`) at replay time. The bridge cannot solve this on its own — the broadcast model is correct for observability subscribers (UI, log streamers, debug tools) but wrong as the sole pipe for durable persistence.

Mitigations available to M5 (not enforced by M3):

1. **Dedicated unblocked subscriber task.** M5 engine runs its persistence subscriber in a `tokio::task::spawn` that does no blocking work — it reads from the receiver, sends to an internal mpsc that the storage writer drains. The broadcast→mpsc bounce is the standard pattern for "broadcast as durable feed".
2. **Treat `Lagged` as fatal.** M5 aborts the run on `Lagged`, marks the run as `RunStatus::Crashed`, surfaces a diagnostic. Catches lag bugs early instead of letting them corrupt long-running views.
3. **Future M3.5 enhancement (out of scope today):** add a separate `mpsc` "primary subscriber" channel that bypasses broadcast. Bridge emits each event to mpsc *and* broadcast; mpsc is single-consumer for the engine, broadcast is for observability. Only worth doing if M5 demonstrates the mitigation above is insufficient — bridge complexity grows non-trivially.

**M3 contract:** broadcast is best-effort observability; integrators that need durable consumption design their own backpressure. Documented prominently in `AcpBridge::subscribe` rustdoc with a link back to this section.

## 12. Realistic effort estimate

Per the M2 calibration (M2 ran ~50% over its 2-week budget), and given M3's surface (~14 new modules including `shared/`, 13 integration tests, a from-scratch mock ACP binary, a new `Sandbox` trait surface, and verification of an unstable SDK feature), realistic estimate is **3–4 weeks of solo evening/weekend work**.

Likely surprise sinks:

- ACP 0.10.2 tool-injection surface differs from the architecture doc's prose (open question §11.1).
- Mock-agent binary needs more scenarios than enumerated to cover real test pain.
- Cross-platform stderr draining timing (Windows pipes behave subtly differently).
- `Sandbox` trait shape needs one revision after writing the first M4-style stub impl (acceptance #14).

Build buffer in. Do not commit to a 2-week shipping date.

## 13. Open questions for implementation phase

These are for the writing-plans phase, not blockers for design approval:

### 13.1 Exact ACP 0.10.2 tool-injection surface

To be answered by reading `agent-client-protocol = 0.10.2` rustdoc and source. Determines whether `tools.rs` declares tools via a struct field or a callback registration. Spec updated if surface materially differs.

### 13.2 `Sandbox` trait: sync vs async methods

The trait in §4.6 uses sync methods. M4's `LandlockSandbox` may need async (e.g., to query a cached policy file). Two options:

- Keep sync now; M4 wraps async work in `tokio::task::block_in_place` if needed.
- Switch to `async-trait` now; bridge just `.await`s decisions.

Sync is simpler and avoids an `async-trait` dep. Decision: **sync now**, revisit in M4 if Landlock impl forces it. The trait can grow a parallel `async fn allows_tool_async` later without breaking M3.

### 13.3 `MessageContent::Blocks` handling in mock

The mock currently echoes text. Whether it round-trips full `Vec<ContentBlock>` (including images, audio) depends on whether tests need that coverage. Default: mock handles text only; an image/audio test is added if M5 engine needs the round-trip.

### 13.4 `BridgeEvent::HumanInputRequested` reply path

M3 ships the event but no reply API (per §5.3). The reply API (`AcpBridge::reply_to_human_input(call_id, payload)`) is M5 scope. Open question for the plan phase: do we ship a stub method on `AcpBridge` in M3 that returns `Unimplemented`, or do we leave it absent and add cleanly in M5? Default: leave absent (smaller M3 surface, no breaking change in M5).

### 13.5 Whether to emit `BridgeEvent::TokenUsage` on every chunk or coalesce

If ACP reports usage on every chunk, the event volume could be high. Coalescing window (e.g., emit at most once per 250 ms) saves event-broadcast bandwidth at the cost of slight latency. Default: emit on every report — broadcast is cheap, M5 can coalesce in its writer if needed.

### 13.6 CI matrix for the mock binary

Plan phase decides whether the mock binary is built only when `cargo test -p surge-acp` runs (default), or as part of `cargo build --workspace` (slightly slower full builds but always available for ad-hoc CLI testing). Lean toward test-only.
