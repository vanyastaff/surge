# M7 — Daemon Mode + MCP Server Delegation (Design)

**Status:** Design (drafted 2026-05-04)
**Predecessor:** M6 (frame stack + Notify + in-process CLI — shipped, see
[`2026-05-04-surge-orchestrator-engine-m6-design.md`](2026-05-04-surge-orchestrator-engine-m6-design.md))
**Successor:** M8 (retry, bootstrap stages, HumanGate channels — see
M6 spec §22.3)

> **Scope summary.** M7 ships two coupled extensions of the M6
> in-process engine:
>
> 1. A long-running **`surge daemon`** process that hosts the engine
>    and accepts IPC commands from the CLI, allowing runs to survive
>    terminal close and multiple CLI invocations to share state.
> 2. **MCP server delegation** that lets agent stages call tools
>    beyond the engine's built-in `read_file` / `shell_exec` surface
>    by routing tool calls to user-configured MCP servers
>    (subprocess stdio in M7; remote HTTP transports deferred).
>
> Both subsystems share the same `EngineFacade` trait abstraction
> and the same calibration window, so they ship in a single
> milestone (validated against `feedback_spec_scope_discipline.md`'s
> "split if >3 subsystems" rule — see §22 self-review).

---

## 1. Goals and non-goals

### 1.1 In scope

- **Long-running daemon process** (`surge-daemon` binary). Hosts an
  `Engine` instance, accepts IPC connections from CLI clients, drives
  one or more concurrent runs.
- **Cross-platform IPC** over local sockets — Unix domain sockets on
  Linux/macOS, named pipes on Windows. JSON-RPC 2.0 with line-delimited
  framing on top.
- **`EngineFacade` trait** with two implementations: `LocalEngineFacade`
  (M6 in-process behaviour, default) and `DaemonEngineFacade` (forwards
  every method as an IPC request).
- **`AdmissionController`** — caps concurrent active runs (default 8),
  enqueues additional `start_run` requests in a FIFO queue. No aging,
  no preemption; both deferred to M8 if real workloads need them.
- **`BroadcastRegistry`** — multi-subscriber event fan-out. The daemon
  may have several `surge engine watch <run_id>` clients tailing the
  same run; each gets an independent broadcast subscription.
- **`--daemon` flag retrofit** on `surge engine run|resume|stop|watch|
  ls|logs`. With `--daemon`, the CLI auto-spawns the daemon if not
  running and routes the call through `DaemonEngineFacade`. Without
  `--daemon`, M6 behaviour is preserved verbatim.
- **`surge daemon start|stop|status|restart`** subtree for explicit
  daemon lifecycle control.
- **MCP server delegation** via the official [`rmcp`](https://docs.rs/rmcp)
  crate (`>=1.6`). M7 supports the `transport-child-process` (stdio)
  transport — the most common case where the user has installed an
  MCP server binary like `mcp-server-playwright` or `mcp-server-github`.
- **`McpRegistry`** in a new `crates/surge-mcp/` crate that owns the
  per-server connection state (lazy connect on first use, reconnect
  policy, in-flight call tracking).
- **`RoutingToolDispatcher`** in `crates/surge-orchestrator/src/engine/
  tools/routing.rs` that implements `ToolDispatcher` by fanning out:
  engine-built-in tools to `WorktreeToolDispatcher`, MCP tools to
  the configured `McpRegistry`.
- **Sandbox-aware tool exposure** — at session-open time, the
  `RoutingToolDispatcher` returns a tool list filtered by the active
  sandbox tier (`SandboxMode`) and the agent stage's `mcp_servers`
  whitelist.
- **`AgentConfig::mcp_servers: Vec<McpServerRef>`** — new field on
  `AgentConfig` (additive, default empty). Per-stage override of which
  MCP servers are visible to the agent.
- **Per-server crash recovery** — if the MCP server child process
  exits mid-call, the in-flight call returns `McpServerCrashed`, the
  engine does not retry, and the next call triggers a re-spawn (per
  `restart_on_crash`, default `true`).
- **Lock file + socket path discovery** — `~/.surge/daemon/daemon.pid`
  and `~/.surge/daemon/daemon.sock` (or named pipe path on Windows).
  Stale lock detection on daemon startup.
- **Graceful shutdown** — daemon handles `SIGTERM`/`SIGINT` (Unix) and
  `Ctrl+C` (Windows) by signalling all active runs to stop, draining
  to a stable snapshot boundary, then exiting.

### 1.2 Out of scope (deferred)

- **Aging / preemption in `AdmissionController`** — M8 if interactive
  workloads suffer from FIFO blocking.
- **Daemon authentication / multi-user.** M7's daemon is single-user;
  the socket has user-only permissions (`0700` parent dir on Unix,
  default ACL on Windows). Multi-user RPC is M9+.
- **Hot-reload of MCP server config.** M7 reads the config at run
  start; changes require restart of the run (not the daemon).
  Hot-reload deferred unless real users complain.
- **MCP `resources` and `prompts`.** M7 wires only `tools/list` and
  `tools/call`. Resources and prompts are defined in the MCP spec
  but out of scope here; they don't fit the `ToolDispatcher` contract
  and would need their own surface.
- **HTTP / SSE MCP transports.** M7 does subprocess stdio (the
  `transport-child-process` rmcp feature) only. `transport-streamable-
  http-client` and SSE deferred to M7+ when there's a real driver.
- **Daemon binary distribution / packaging.** M7 ships `cargo run -p
  surge-daemon` and `cargo install --path crates/surge-daemon`; system
  service files (`systemd`, launchd plist, Windows Service) are M9+.
- **Bootstrap stages, retry, HumanGate channels.** Same scope as M6
  §22.3 — owned by M8.
- **`NodeKind::Parallel`.** Same status as M6 §22.4 — M8+ owns.
- **`LoopConfig::gate_after_each` channel.** M6 spec §22.5 placed
  this in M7. **Re-evaluating in this spec:** the daemon and broadcast
  registry are in scope here, but adding `LoopConfig::gate_channel:
  Option<NotifyChannel>` plus the inter-iteration synthesis adds a
  third subsystem (gate suspension + reply routing). Per
  `feedback_spec_scope_discipline.md` "split if >3 subsystems",
  **promoting `gate_after_each` to M8** alongside the other HumanGate
  delivery work keeps M7 at the daemon+MCP boundary. M6's validation
  rejection stays; M8 lifts it.
- **Snapshot v3.** No need — M7 doesn't change run state. The
  daemon is a host process, not a state owner; existing v2 snapshots
  flow through unchanged.
- **Per-run subprocess hosting.** The canonical revision §03-engine
  models the daemon as one OS process per run. Surge has chosen
  one-daemon-hosts-many-runs since M5 (see §23 accepted divergence).
  M7 preserves this — no per-run subprocess work.

---

## 2. Why this milestone — context and motivation

### 2.1 The "runs die with the CLI" problem

After M6, `surge engine run flow.toml --watch` works in-process: the
engine spawns inside the CLI's tokio runtime, executes nodes, streams
events to stderr, and exits when the run terminates. The moment the
CLI exits — the run dies. There is no mechanism to start a long-running
flow and walk away from the terminal.

This is the M7 daemon's first job: detach run lifetime from CLI
lifetime. After M7, this works:

```bash
surge engine run flow.toml --daemon       # spawns daemon, returns run_id, exits
surge engine watch <run_id>               # tails events from any new shell
surge engine ls                           # lists active runs
surge engine stop <run_id>                # cancels remotely
```

### 2.2 The "agents are stuck with engine tools" problem

After M6, agent stages can only call the four tools the engine
publishes: `read_file`, `write_file`, `shell_exec`, `apply_diff`
(plus the two engine-handled meta-tools `report_stage_outcome` and
`request_human_input`). Real workflows need much more — for example:

- **`mcp-server-playwright`** for a research/QA stage that drives a
  browser.
- **`mcp-server-github`** for the `pr-composer` profile (creates PRs,
  reads issues).
- **`mcp-server-postgres`** for a migration verification stage.
- **`mcp-server-memory`** for a long-running run to persist context
  across stages.

The M7 MCP delegation lets users configure these in their
`AgentConfig::mcp_servers` (or the per-profile default), and the
`RoutingToolDispatcher` exposes them transparently to the agent at
session open. This is the canonical revision §04-acp-integration §"2.
Sandbox-filtered MCP tools" surface, made real.

### 2.3 Coupling rationale (single milestone)

Daemon and MCP look orthogonal — and they almost are. They couple at
exactly two points:

1. **The daemon must own the `McpRegistry`.** MCP server child
   processes have lifetimes longer than a single ACP session: a
   `playwright` server stays warm across stages and across runs. If
   each `surge engine run` invocation spawned its own MCP servers,
   we'd lose that warmth (bad UX, bad cost). The daemon is the
   correct lifetime owner.
2. **The `EngineFacade` trait is shared.** Both `--daemon` mode and
   the local-only mode go through the same trait, so engine code
   doesn't fork. Adding MCP later would require touching the same
   `EngineFacade` for tool exposure during session open (the agent
   stage has to know which MCP tools to declare).

Splitting M7 into M7a (daemon only, MCP-less) followed by M7b (MCP)
would mean either (a) shipping M7a with no MCP code paths and then
threading them through later (rework), or (b) shipping M7a with
inert MCP scaffolding that does nothing (decide-or-defer violation).
Per memory `feedback_spec_scope_discipline.md` rule 3 ("decide
implement OR defer, never half-implement"), single-milestone is the
right call.

The ship-as-PR cadence within M7 (§18) still gives intermediate
checkpoints: PR 1-3 land daemon-only with no MCP wiring; PR 4-5
add MCP on top. If life eats the second half, the first half
already shipped useful functionality.

---

## 3. Architecture decisions

### 3.1 Single-process daemon hosts many runs

Per the M5-era decision documented in
[`2026-05-03-surge-orchestrator-engine-m5-design.md`](2026-05-03-surge-orchestrator-engine-m5-design.md)
§14 and the canonical revision divergence acknowledged in memory
`feedback_consult_revision_docs.md`, surge's daemon hosts an `Engine`
in a single process. The `Engine` already supports concurrent runs
(`tokio::spawn`-per-run, broadcast events, per-run resolution
channels). M7 wraps that engine in an IPC server.

Revision §03-engine.md proposes one OS process per run with `setsid`
on Unix and `DETACHED_PROCESS` on Windows. Surge does not adopt
this — see §23 accepted divergence for rationale and consequences.

### 3.2 IPC: JSON-RPC 2.0 over `interprocess::local_socket` with line-delimited framing

**Crate:** [`interprocess`](https://docs.rs/interprocess) `>=2` with
the `tokio` feature flag. The crate's `local_socket` module abstracts
"named pipe on Windows, Unix domain socket on Unix" behind a single
async API.

**Framing:** newline-delimited JSON. Each frame is a single JSON-RPC
2.0 message terminated by `\n`. JSON serialisation via `serde_json`
in compact mode (no embedded newlines). This matches the framing the
ACP bridge already uses against agent subprocesses, so the precedent
exists and is debugged.

**Why JSON-RPC and not custom?** JSON-RPC 2.0 has a notification
form (no response expected — perfect for `DaemonEvent` broadcasts),
a request/response form with correlation IDs, and standardised error
codes. Hand-rolling a binary framing would require us to invent the
correlation logic ourselves and is harder to debug with `socat` or
`nc`. The trade-off is larger bytes-on-wire, but on a local socket
this is irrelevant.

**Why `interprocess` and not `parity-tokio-ipc`?** Both work. We
pick `interprocess` for active maintenance (2.4.2 in 2026), broader
docs, and a unified `local_socket` namespace that feels like
`tokio::net::TcpStream`. Decision is reversible — see §16.

### 3.3 `EngineFacade` trait abstracts in-process vs daemon

```rust
#[async_trait]
pub trait EngineFacade: Send + Sync {
    async fn start_run(&self, ...) -> Result<RunHandle, EngineError>;
    async fn resume_run(&self, ...) -> Result<RunHandle, EngineError>;
    async fn stop_run(&self, run_id: RunId, reason: String) -> Result<(), EngineError>;
    async fn resolve_human_input(&self, ...) -> Result<(), EngineError>;
    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError>;
}
```

Two impls:

- **`LocalEngineFacade`** wraps an in-process `Engine` and forwards
  every call directly. M6 behaviour preserved verbatim.
- **`DaemonEngineFacade`** opens an IPC connection to the daemon and
  forwards every call as a JSON-RPC request. The returned `RunHandle`
  has its broadcast `Receiver` plumbed across the socket via a
  separate "subscribe_events" subscription channel.

The CLI picks one based on `--daemon`. M6 in-process tests use
`LocalEngineFacade`; M7 daemon-mode tests use `DaemonEngineFacade`
with an embedded daemon thread (no real subprocess).

### 3.4 MCP via the official `rmcp` crate

**Crate:** [`rmcp`](https://docs.rs/rmcp) `>=1.6` with features
`["client", "transport-child-process"]`. This is the
[`modelcontextprotocol/rust-sdk`](https://github.com/modelcontextprotocol/rust-sdk)
official SDK.

**Why rmcp?** It's the canonical Rust SDK (4.7M+ downloads as of
2026-05), Send-friendly futures (no `LocalSet` bridge needed unlike
ACP), supports stdio child-process transport out of the box via
`TokioChildProcess`, integrates with `tokio` 1.x. Releases are
frequent and active.

**Why not hand-roll JSON-RPC against the MCP wire format?** We'd
re-implement the protocol's capability negotiation, tools listing,
content-type marshalling, and error model. ~2000 lines of code we
shouldn't own. rmcp owns it; we benefit from upstream fixes.

**Why not `mcp-sdk` / `mcpr` / `agenterra-rmcp`?** Less adoption,
less doc coverage. rmcp is the consensus pick in the 2026 ecosystem.
If rmcp goes unmaintained later (low risk given current activity),
swapping is a contained refactor inside `crates/surge-mcp/`.

### 3.5 `RoutingToolDispatcher` wraps the M6 surface

```rust
pub struct RoutingToolDispatcher {
    engine_dispatcher: Arc<dyn ToolDispatcher>,   // M6's WorktreeToolDispatcher
    mcp_registry: Arc<McpRegistry>,
    routing_table: HashMap<String, ToolOrigin>,   // tool name -> origin
}

enum ToolOrigin {
    Engine,
    Mcp { server_name: String },
}
```

The `routing_table` is built at session-open time from the engine's
built-in tool catalog plus the `tools/list` response from each
configured MCP server. Tool name collisions are resolved by
preferring engine tools (so a user can't accidentally shadow
`shell_exec` with an MCP server).

`RoutingToolDispatcher` implements the existing
[`ToolDispatcher`](crates/surge-orchestrator/src/engine/tools/mod.rs)
trait, so it slots into the existing `Engine` constructor without
trait surgery. M6's `WorktreeToolDispatcher` becomes the
`engine_dispatcher` field of the routing dispatcher when M7 wires
the production CLI.

### 3.6 Snapshot schema unchanged (still v2)

The daemon does not introduce new run-state. Active runs persist via
the existing `EngineSnapshot` v2 mechanism, which already handles
frame stacks and traversal counters from M6. Daemon restart on the
same `~/.surge` directory will scan, find non-terminal runs by their
last persisted seq, and offer (per `surge daemon start --resume-all`)
to resume them — but this is bookkeeping, not schema work.

If a daemon is upgraded between M6 and M7, M6 v2 snapshots flow
through unchanged. Deserialisation already has the v1→v2 transparent
upgrade path; M7 adds nothing.

### 3.7 `AdmissionController`: FIFO without aging

```rust
pub struct AdmissionController {
    max_active: usize,             // default 8
    active: HashSet<RunId>,
    queue: VecDeque<PendingAdmission>,
    notify: tokio::sync::Notify,
}
```

When the daemon receives `start_run` and `active.len() >= max_active`,
the request joins `queue`. As runs terminate (`Engine::stop_run`,
natural completion), the daemon dequeues the head and admits it.

No priorities, no aging, no preemption. M8 owns those if a real
workload needs them. Strict FIFO with a documented `max_active` knob
is the simplest thing that works for solo use of M7.

### 3.8 `BroadcastRegistry`: per-run + global subscriptions

```rust
pub struct BroadcastRegistry {
    per_run: HashMap<RunId, broadcast::Sender<EngineRunEvent>>,
    global: broadcast::Sender<DaemonEvent>,
}

pub enum DaemonEvent {
    RunAccepted { run_id: RunId },
    RunFinished { run_id: RunId, outcome: RunOutcome },
    DaemonShuttingDown,
}
```

The daemon hosts one broadcast channel per active run plus a single
"global" channel for daemon-level events. CLI clients subscribe to
the per-run channel for `surge engine watch <run_id>` and to the
global channel for `surge engine ls --watch` (post-M7 streaming
listing — listed in §16 as a soft target).

### 3.9 PID file + socket file for daemon discovery

```
~/.surge/daemon/
├── daemon.pid          (PID of the running daemon)
├── daemon.sock         (Unix socket; on Windows: a placeholder file
│                        whose contents is the named pipe path)
└── version             (text: "0.1.0" — the daemon binary version)
```

CLI flow when `--daemon` is requested:

1. Read `daemon.pid`. If absent, daemon is not running.
2. Read PID and check if the process is alive (cross-platform via
   `sysinfo`).
3. If alive, read `daemon.sock` to learn the socket / pipe path.
4. Connect to the socket. If connect fails after the alive check,
   the daemon may be in shutdown — wait briefly and retry once,
   then bail.
5. If absent or stale, `--daemon` triggers an auto-spawn:
   `surge daemon start --detached` (which forks/spawns a child
   process that detaches from the controlling terminal).

The version file lets the CLI bail with a clear error if the daemon
binary is older than the CLI binary (mismatched event payload
serdes). M7 ships a single version; the version negotiation matters
in M8+ when the surface evolves.

### 3.10 Graceful shutdown

On `SIGTERM` / `SIGINT` (Unix) or `Ctrl+C` (Windows), the daemon:

1. Stops accepting new IPC connections.
2. Sends `DaemonEvent::DaemonShuttingDown` to the global broadcast.
3. For each active run, calls `Engine::stop_run(run_id, "daemon
   shutdown")`. The engine emits `RunAborted` and the run's task
   exits.
4. Waits up to a `shutdown_grace` duration (default 30s) for all run
   tasks to finish.
5. For any MCP server child processes still alive, sends them a
   close request (rmcp's `RunningService::cancel`).
6. Removes `daemon.pid` and `daemon.sock`.
7. Exits with code 0.

If `shutdown_grace` elapses before runs finish, the daemon logs each
stragglers and force-exits. Stale snapshots from those runs remain
on disk; `surge daemon start --resume-all` can pick them up later.

---

## 4. Module layout

### 4.1 New crate: `crates/surge-daemon/`

```
crates/surge-daemon/
├── Cargo.toml          (lib + bin)
├── README.md           (operator-facing setup, troubleshooting)
└── src/
    ├── lib.rs          (re-exports)
    ├── main.rs         (binary entry point)
    ├── server.rs       (IPC server loop, accept-and-dispatch)
    ├── admission.rs    (AdmissionController)
    ├── broadcast.rs    (BroadcastRegistry)
    ├── lifecycle.rs    (signal handlers, graceful shutdown)
    ├── pidfile.rs      (PID file + socket discovery, stale detection)
    └── error.rs        (DaemonError)
```

Why `lib + bin`? Lib lets us write daemon-server tests in
`crates/surge-daemon/tests/` that spin up the server in the same
process and bypass the CLI altogether. Bin is just `main()` that
parses flags, builds an `Engine`, hands it to `lib::server::run`.

### 4.2 New crate: `crates/surge-mcp/`

```
crates/surge-mcp/
├── Cargo.toml
├── README.md           (per-server setup, MCP server install pointers)
└── src/
    ├── lib.rs
    ├── config.rs       (McpServerRef, McpTransportConfig, etc.)
    ├── connection.rs   (McpServerConnection — wraps rmcp RunningService)
    ├── registry.rs     (McpRegistry)
    └── error.rs        (McpError)
```

Mirrors the surge-notify pattern. rmcp lives only here, isolated from
the orchestrator.

### 4.3 Extensions to `crates/surge-orchestrator/`

```
crates/surge-orchestrator/src/engine/
├── facade.rs           (NEW: EngineFacade trait, LocalEngineFacade,
│                            DaemonEngineFacade)
├── ipc.rs              (NEW: DaemonRequest, DaemonResponse,
│                            DaemonEvent enums + framing helpers)
├── tools/
│   ├── mod.rs          (EXTENDED: `ToolDispatcher::declared_tools`
│                            default-method, `DeclaredTool` type)
│   └── routing.rs      (NEW: RoutingToolDispatcher impl)
```

The engine itself is unchanged. New code is purely additive. The
`ToolDispatcher` trait grows one method:

```rust
#[async_trait]
pub trait ToolDispatcher: Send + Sync {
    async fn dispatch(&self, ctx: &ToolDispatchContext<'_>, call: &ToolCall) -> ToolResultPayload;

    /// Tools this dispatcher offers to agent stages. Default returns
    /// empty (M6 dispatchers don't need to opt in). M7's
    /// `WorktreeToolDispatcher` overrides to declare its built-in tools.
    fn declared_tools(&self) -> Vec<DeclaredTool> { Vec::new() }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DeclaredTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}
```

The default impl makes the change non-breaking for existing impls.
`WorktreeToolDispatcher` is updated in Phase 2 to override and return
its real tool catalog (`read_file`, `write_file`, `shell_exec`,
`apply_diff`).

Existing tests continue to compile (additive changes). All new
public enums get `#[non_exhaustive]` — see §22 self-review.

### 4.4 Extensions to `crates/surge-cli/`

```
crates/surge-cli/src/commands/
├── daemon.rs           (NEW: surge daemon start|stop|status|restart)
├── engine.rs           (EXTENDED: --daemon flag on each subcommand)
```

The `--daemon` flag in `engine.rs` switches the CLI from constructing
a `LocalEngineFacade` to a `DaemonEngineFacade`. M6 behaviour is the
default; M7 adds a path that auto-spawns the daemon if requested and
not running.

### 4.5 Extensions to `crates/surge-core/`

```
crates/surge-core/src/
├── agent_config.rs     (EXTENDED: AgentConfig::mcp_servers field)
├── mcp_config.rs       (NEW: McpServerRef serialisable type — shared
│                            with surge-mcp via re-export)
├── validation.rs       (EXTENDED: M7 validation — see §10.5)
```

Core extensions are additive: new field with `default = Vec::new()`,
new type, validation rule.

---

## 5. Public API

### 5.1 `EngineFacade` trait

```rust
// crates/surge-orchestrator/src/engine/facade.rs

#[async_trait]
pub trait EngineFacade: Send + Sync {
    async fn start_run(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError>;

    async fn resume_run(
        &self,
        run_id: RunId,
        worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError>;

    async fn stop_run(
        &self,
        run_id: RunId,
        reason: String,
    ) -> Result<(), EngineError>;

    async fn resolve_human_input(
        &self,
        run_id: RunId,
        call_id: Option<String>,
        response: serde_json::Value,
    ) -> Result<(), EngineError>;

    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError>;
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct RunSummary {
    pub run_id: RunId,
    pub status: RunStatus,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub last_event_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum RunStatus {
    Active,
    Awaiting,         // queued by AdmissionController, not yet started
    Completed,
    Failed,
    Aborted,
}
```

`Engine::start_run` already returns `RunHandle`, which contains a
`broadcast::Receiver<EngineRunEvent>`. For `LocalEngineFacade` this
is the same in-process receiver M6 returns. For `DaemonEngineFacade`,
the receiver is fed by a per-run subscription task that forwards
events from the daemon's IPC stream into a fresh in-process broadcast
channel that the CLI hands back to its caller.

### 5.2 `LocalEngineFacade`

```rust
pub struct LocalEngineFacade {
    engine: Arc<Engine>,
}

impl LocalEngineFacade {
    pub fn new(engine: Arc<Engine>) -> Self { Self { engine } }
}

#[async_trait]
impl EngineFacade for LocalEngineFacade {
    async fn start_run(&self, ...) -> Result<RunHandle, EngineError> {
        self.engine.start_run(...).await
    }
    // ... straight delegation
}
```

Pure delegation. No new functionality. Lets engine-construction code
hold an `Arc<dyn EngineFacade>` and be unaware of where the engine
runs.

### 5.3 `DaemonEngineFacade`

```rust
pub struct DaemonEngineFacade {
    inner: Arc<DaemonClient>,
}

struct DaemonClient {
    socket_path: PathBuf,
    write_tx: mpsc::Sender<DaemonRequest>,
    pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<DaemonResponse>>>>,
    event_dispatcher: EventDispatcher,
}

impl DaemonEngineFacade {
    pub async fn connect(socket_path: PathBuf) -> Result<Self, EngineError> { ... }
}
```

`connect` opens the local socket, spawns a background task that
reads lines, parses each as `DaemonResponse` or `DaemonEvent`, and
dispatches:
- `DaemonResponse` → look up the `RequestId` in `pending`, send via
  the `oneshot::Sender`.
- `DaemonEvent::PerRun { run_id, event }` → forward to the per-run
  broadcast channel held by the `EventDispatcher`.

When `DaemonEngineFacade::start_run` is called, it:
1. Allocates a fresh `RequestId`.
2. Sends `DaemonRequest::StartRun { request_id, run_id, graph,
   worktree_path, run_config }` over the socket.
3. Awaits the response on the matching oneshot.
4. On `DaemonResponse::StartRunOk { run_id }`, opens a per-run
   broadcast channel, sends `DaemonRequest::Subscribe { run_id }`,
   and returns a `RunHandle` whose `events` is the receiver of that
   channel and whose `completion` is a `JoinHandle` waiting for
   `DaemonEvent::PerRun { event: Terminal(...) }`.

### 5.4 IPC protocol

**Request envelope:**

```rust
#[derive(serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum DaemonRequest {
    Ping { request_id: RequestId },
    StartRun {
        request_id: RequestId,
        run_id: RunId,
        graph: Box<Graph>,            // boxed — Graphs can be ~500 KB
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    },
    ResumeRun {
        request_id: RequestId,
        run_id: RunId,
        worktree_path: PathBuf,
    },
    StopRun { request_id: RequestId, run_id: RunId, reason: String },
    ResolveHumanInput {
        request_id: RequestId,
        run_id: RunId,
        call_id: Option<String>,
        response: serde_json::Value,
    },
    ListRuns { request_id: RequestId },
    Subscribe { request_id: RequestId, run_id: RunId },
    Unsubscribe { request_id: RequestId, run_id: RunId },
    Shutdown { request_id: RequestId },
}

#[derive(serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum DaemonResponse {
    PingOk { request_id: RequestId, version: String },
    StartRunOk { request_id: RequestId, run_id: RunId },
    StartRunQueued { request_id: RequestId, run_id: RunId, position: usize },
    ResumeRunOk { request_id: RequestId },
    StopRunOk { request_id: RequestId },
    ResolveHumanInputOk { request_id: RequestId },
    ListRunsOk { request_id: RequestId, runs: Vec<RunSummary> },
    SubscribeOk { request_id: RequestId },
    UnsubscribeOk { request_id: RequestId },
    ShutdownOk { request_id: RequestId },
    Error { request_id: RequestId, code: ErrorCode, message: String },
}

#[derive(serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum DaemonEvent {
    PerRun { run_id: RunId, event: EngineRunEvent },
    Global(GlobalDaemonEvent),
}

#[derive(serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum GlobalDaemonEvent {
    RunAccepted { run_id: RunId },
    RunFinished { run_id: RunId, outcome: RunOutcome },
    DaemonShuttingDown,
}
```

**Framing:** one JSON object per line. Server side reads with
`tokio::io::BufReader::lines`; client side same. Compact serialisation
guarantees no embedded newlines.

`request_id` is a `u64` allocated client-side. `RunId` is the
existing `surge_core::id::RunId` (ULID).

`StartRunQueued` is returned when the `AdmissionController` queues
the request rather than admitting it. The CLI surface shows this
with a "queued at position N" message but otherwise behaves like
`StartRunOk` — the eventual `RunAccepted` global event triggers the
real start, and the per-run subscription begins receiving events
naturally.

### 5.5 `McpServerRef` config

```rust
// crates/surge-core/src/mcp_config.rs

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[non_exhaustive]
pub struct McpServerRef {
    /// Identifier used in `AgentConfig::mcp_servers` allowlists.
    pub name: String,
    pub transport: McpTransportConfig,
    /// Optional whitelist of tool names. If `None`, all tools the
    /// server reports via `tools/list` are exposed.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Maximum time a single `tools/call` may take. Default 60s.
    #[serde(default = "McpServerRef::default_call_timeout")]
    pub call_timeout: std::time::Duration,
    /// Whether the daemon should re-spawn the server child process if
    /// it exits while this server is still configured. Default true.
    #[serde(default = "McpServerRef::default_restart_on_crash")]
    pub restart_on_crash: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpTransportConfig {
    /// Spawn `command args` and talk MCP over its stdio. The most
    /// common case (every official MCP server in the registry uses
    /// this).
    Stdio {
        command: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    // HTTP / SSE deferred to M7+ (see §1.2).
}
```

`McpServerRef` lives in `surge-core` so the editor (M9+) and the
validator can read graph TOML containing it without depending on
`surge-mcp` (which pulls in rmcp). `surge-mcp` re-exports the type
for convenience.

### 5.6 `McpRegistry`

```rust
// crates/surge-mcp/src/registry.rs

pub struct McpRegistry {
    servers: HashMap<String, Arc<McpServerConnection>>,
}

impl McpRegistry {
    pub async fn from_config(servers: &[McpServerRef]) -> Result<Self, McpError>;

    /// Lazily ensure a connection to `name` is established, then
    /// invoke `tools/call`. The daemon owns one `Arc<McpRegistry>`
    /// shared across all runs.
    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
        timeout: Duration,
    ) -> Result<McpToolResult, McpError>;

    /// Combined `tools/list` across all servers, returned as a
    /// flat catalog. Used by `RoutingToolDispatcher` at session
    /// open.
    pub async fn list_all_tools(&self) -> Result<Vec<McpToolEntry>, McpError>;
}

pub struct McpToolEntry {
    pub server: String,
    pub tool: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

pub struct McpToolResult {
    pub content: Vec<McpContent>,
    pub is_error: bool,
}
```

`McpServerConnection` (private) holds an
`rmcp::service::RunningService<RoleClient>`. On crash detection
(transport returns `None` from `receive`), the connection notes the
crash, the in-flight `oneshot` resolves with `McpServerCrashed`, and
the next `call_tool` triggers re-spawn (subject to `restart_on_crash`).

### 5.7 `RoutingToolDispatcher`

```rust
// crates/surge-orchestrator/src/engine/tools/routing.rs

pub struct RoutingToolDispatcher {
    engine_dispatcher: Arc<dyn ToolDispatcher>,
    mcp_registry: Arc<surge_mcp::McpRegistry>,
    /// Snapshot of the routing table built when the dispatcher was
    /// constructed. Keys are tool names; values say where a call
    /// should land.
    routing_table: HashMap<String, ToolOrigin>,
}

#[derive(Clone, Debug)]
enum ToolOrigin {
    Engine,
    Mcp { server: String },
}

#[async_trait]
impl ToolDispatcher for RoutingToolDispatcher {
    async fn dispatch(
        &self,
        ctx: &ToolDispatchContext<'_>,
        call: &ToolCall,
    ) -> ToolResultPayload {
        match self.routing_table.get(&call.tool) {
            Some(ToolOrigin::Engine) => {
                self.engine_dispatcher.dispatch(ctx, call).await
            }
            Some(ToolOrigin::Mcp { server }) => {
                match self.mcp_registry.call_tool(
                    server,
                    &call.tool,
                    call.arguments.clone(),
                    /* timeout */ Duration::from_secs(60),
                ).await {
                    Ok(result) if !result.is_error => {
                        ToolResultPayload::Ok {
                            content: convert_mcp_content(result.content),
                        }
                    }
                    Ok(result) => {
                        ToolResultPayload::Error {
                            message: stringify_mcp_content(result.content),
                        }
                    }
                    Err(McpError::ServerCrashed { .. }) => {
                        ToolResultPayload::Error {
                            message: format!(
                                "MCP server '{server}' crashed mid-call"
                            ),
                        }
                    }
                    Err(e) => ToolResultPayload::Error {
                        message: format!("MCP error: {e}"),
                    },
                }
            }
            None => ToolResultPayload::Unsupported {
                message: format!("unknown tool: {}", call.tool),
            },
        }
    }
}
```

The routing table is constructed before the dispatcher is registered
on a session: the engine asks `mcp_registry.list_all_tools()`,
filters by sandbox + the active stage's `mcp_servers` allowlist, and
inserts each (tool_name, origin) pair into the table. Engine-built-in
tools are inserted last (overwriting any MCP-side collisions — see
§3.5).

### 5.8 `AgentConfig::mcp_servers`

```rust
// crates/surge-core/src/agent_config.rs

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[non_exhaustive]
pub struct AgentConfig {
    // ... existing fields
    /// MCP servers exposed to this agent stage. Empty = no MCP tools.
    /// Each entry must reference a server defined in the run-level
    /// `RunConfig::mcp_servers` registry.
    #[serde(default)]
    pub mcp_servers: Vec<String>,
}
```

The actual `McpServerRef` definitions live in a run-level config
(carried by `EngineRunConfig` or a new `RunConfig::mcp_servers`
field — see §5.5 for placement). Per-stage the agent declares which
ones it wants visible; the engine intersects with sandbox restrictions
and the stage's profile defaults.

For M7's stage scope, we keep it simple: `AgentConfig::mcp_servers`
holds *names*; the run config holds the registry of `McpServerRef`s.
This mirrors how `surge-notify` handled per-channel config (the
multiplexer holds the global registry; nodes reference channels by
name).

---

## 6. Run lifecycle through daemon

### 6.1 Daemon startup

```
$ surge daemon start
[surge-daemon 0.1.0] starting…
[surge-daemon] socket: /home/user/.surge/daemon/daemon.sock
[surge-daemon] pid: 12345 (written to /home/user/.surge/daemon/daemon.pid)
[surge-daemon] storage: /home/user/.surge
[surge-daemon] mcp servers (configured): 0
[surge-daemon] ready (listening for connections)
```

Steps:
1. Parse `--max-active`, `--shutdown-grace`, `--config <path>` flags.
2. Resolve `~/.surge/daemon/`, create dir if missing.
3. Acquire PID file lock. If another daemon holds it, exit with
   `EAlready` (operator should `surge daemon status`).
4. Open `Storage`, build `BridgeFacade`, build a default
   `RoutingToolDispatcher` (with empty `McpRegistry` initially —
   the registry is populated lazily per-run from `RunConfig`).
5. Build `Engine`. Wrap in `LocalEngineFacade`.
6. Start the IPC server: bind `local_socket::ListenerOptions`,
   accept loop in a tokio task.
7. Install signal handlers (SIGTERM, SIGINT, Ctrl-C).
8. Mark daemon as ready (write `daemon.sock` ready marker).

### 6.2 Client connects

When the CLI runs `surge engine run flow.toml --daemon`:
1. CLI checks `~/.surge/daemon/daemon.pid` and `daemon.sock`.
2. If missing or stale, CLI auto-spawns the daemon (`surge daemon
   start --detached`) and waits up to 5 s for the socket to appear.
3. CLI builds `DaemonEngineFacade::connect(socket_path)`.
4. The connect call opens the socket, reads the version handshake
   (the daemon sends a `Welcome` event on accept), validates that
   the major version matches the CLI's, and returns the facade.

### 6.3 `start_run` flow (CLI → daemon → engine)

```
CLI                    Daemon                       Engine
 |                       |                             |
 | StartRun (req)        |                             |
 |---------------------->|                             |
 |                       | AdmissionController.try_admit
 |                       | (if admitted)               |
 |                       | engine.start_run(graph, …)  |
 |                       |---------------------------->|
 |                       |<-- RunHandle ---------------|
 |                       | broadcast.register(run_id)  |
 |                       | spawn forward_events task   |
 | StartRunOk            |                             |
 |<----------------------|                             |
 | Subscribe (req)       |                             |
 |---------------------->|                             |
 | SubscribeOk           |                             |
 |<----------------------|                             |
 |                       |                             |
 |                       |                             | (engine emits events)
 |                       |<-- broadcast::send ---------|
 | DaemonEvent::PerRun   |                             |
 |<----------------------|                             |
 |   …                                                 |
 |                       |                             | (run terminates)
 |                       |<-- Terminal -----------------|
 | DaemonEvent::PerRun   |                             |
 |  Terminal             |                             |
 |<----------------------|                             |
 |                       | broadcast.deregister(run_id)|
 |                       | admission.notify_completed  |
 | (if --watch ends)     |                             |
 | Unsubscribe           |                             |
 |---------------------->|                             |
```

The `forward_events` task is `tokio::spawn`ed by the daemon for each
admitted run. It loops on `engine_run_handle.events.recv()` and
writes each event into the `BroadcastRegistry`'s per-run channel,
which fan-outs to all subscribers (CLI clients).

### 6.4 `subscribe_events` flow

When a CLI subscribes to a run that's already mid-flight (e.g.,
`surge engine watch <run_id>` after `--daemon` was used in another
shell), the daemon:
1. Looks up the per-run broadcast `Sender`. If missing, the run is
   not active in this daemon — return `Error { code: RunNotActive }`.
2. Constructs a fresh `Receiver` from the `Sender`.
3. Spawns a per-subscriber task that forwards events into a JSON-RPC
   `DaemonEvent::PerRun` notification on the client's connection.
4. Sends `SubscribeOk`.

The CLI client's `EventDispatcher` routes incoming `DaemonEvent`s
into the right per-run broadcast channel it manages internally.

Late subscribers see only events from now on — there is no replay.
For replay, use `surge engine logs <run_id>` which reads from the
on-disk event log via the existing `Storage::open_run_reader`.

### 6.5 `stop_run` flow

```
CLI                    Daemon                    Engine
 |                       |                          |
 | StopRun (req)         |                          |
 |---------------------->|                          |
 |                       | engine.stop_run(run_id)  |
 |                       |------------------------->|
 |                       |<-- () -------------------|
 | StopRunOk             |                          |
 |<----------------------|                          |
 |                       |                          | (run aborts)
 |                       |<-- Terminal(Aborted) ----|
 |                       | (terminal event flows to subscribers)
```

Same as M6's `Engine::stop_run`, but called from the daemon's
request-handler task.

### 6.6 Daemon shutdown (graceful)

See §3.10. Trigger sources: `SIGTERM` (operator), `surge daemon stop`
(IPC `Shutdown` request), `Ctrl+C` if foregrounded. All three converge
on the same drain logic.

### 6.7 Daemon crash recovery on next startup

If the daemon process is killed forcefully (`kill -9`, OOM, power
cut), restart sees a stale `daemon.pid` whose process is no longer
alive. In that case:
1. The new daemon overwrites the stale pid + socket files.
2. It logs a warning: "previous daemon (pid N) appears to have died
   uncleanly".
3. It scans `~/.surge/runs` for runs whose last persisted event is
   not a `Terminal`. These are "orphaned active runs".
4. If `--resume-all` was passed, the new daemon attempts to resume
   each orphaned run via `Engine::resume_run`. Without the flag,
   they sit on disk untouched, and `surge engine ls` shows them
   with status `Aborted (daemon crash)` (synthesised from the
   absence of a terminal event).

Resume is best-effort. A run that crashed mid-stage will, on resume,
re-enter the active node with `attempt+1` per M5's recovery semantics.

---

## 7. MCP delegation

### 7.1 Configuration shape

Run-level (per `~/.surge/runs/{run_id}/config.toml` or the daemon's
default):

```toml
[mcp_servers.playwright]
transport = { kind = "stdio", command = "/usr/local/bin/mcp-playwright" }
allowed_tools = ["browser_navigate", "browser_screenshot"]
call_timeout = "60s"
restart_on_crash = true

[mcp_servers.github]
transport = { kind = "stdio", command = "npx", args = ["@github/mcp-server"] }
```

Per-stage (in `flow.toml`'s agent config):

```toml
[nodes.research_stage]
kind = "Agent"
profile = "researcher@1.0"
mcp_servers = ["playwright"]
```

The agent at `research_stage` sees only the `playwright` server's
tools, not `github`'s. If `mcp_servers` is omitted or `[]`, no MCP
tools are exposed.

### 7.2 Server lifecycle

```
Server state machine (per server, owned by McpRegistry):

  Disconnected --connect()-->  Connecting --rmcp ServiceExt::serve--> Running
       ^                                                                  |
       |                                                                  | (transport returns None)
       |                                                                  v
       +----------------------- Crashed <-------------------------- (auto-restart if configured)
                                  |
                                  | call_tool() while Crashed
                                  v
                              Reconnect (back to Connecting)
```

Lazy connect: the first `call_tool(server, …)` triggers
`ensure_connected(server)`, which spawns the rmcp service and stores
the `RunningService<RoleClient>`. Subsequent calls reuse it.

Crash detection: rmcp's transport returns `None` from
`Transport::receive` when the child process exits. The McpServerConnection's read-loop notices
this, marks state as `Crashed`, fails the in-flight call's
oneshot with `McpError::ServerCrashed`, and resolves to a clean
state.

Reconnection: the next `call_tool` to a `Crashed` server re-runs
`ensure_connected`. If the user disabled `restart_on_crash`, the call
returns `McpError::ServerNotRunning` and the server stays
disconnected until the run is restarted.

### 7.3 Tool exposure to agents

At session-open time (in `engine::stage::agent::execute_agent_stage`,
right where M6 builds the tool list for `BridgeFacade::open_session`):

```rust
let mut tool_list = self.engine_dispatcher.declared_tools();      // e.g. read_file, shell_exec
for srv in &agent_config.mcp_servers {
    let cfg = run_config.mcp_servers.get(srv).ok_or(...)?;
    let server_tools = self.mcp_registry.list_tools_for(srv).await?;
    for entry in server_tools {
        if !cfg.allowed_tools.as_ref().map_or(true, |w| w.contains(&entry.tool)) {
            continue;
        }
        if !sandbox_allows_mcp_tool(&self.sandbox, srv, &entry) {
            continue;
        }
        tool_list.push(convert_to_acp_tool_def(entry));
    }
}
```

The `tool_list` is then handed to `BridgeFacade::open_session` as
`SessionConfig::tools`. The agent literally cannot call tools not in
this list (the bridge enforces the contract).

### 7.4 Routing: engine tool vs MCP tool

When the agent calls a tool, the bridge emits a `BridgeEvent::ToolCall`
with the tool name. M6's existing `engine::stage::agent` looks up the
dispatcher and calls `dispatch(ctx, call)`. With M7's
`RoutingToolDispatcher`, the dispatch lookup matches on the name and
either forwards to the engine dispatcher or to `mcp_registry.call_tool`.

No event-format change. M5's `ToolCalled` / `ToolResultReceived` event
already carries the tool name; we don't add an `mcp_server` field
because the tool name is enough to disambiguate (engine and MCP names
don't collide — see §3.5).

### 7.5 Sandbox interaction at session-open time

The sandbox layer (currently in `engine/sandbox_factory.rs`) is the
authority on whether a particular tool name can be exposed. M7 passes
the same sandbox into `tool_list` construction:

```rust
fn sandbox_allows_mcp_tool(
    sandbox: &SandboxConfig,
    server: &str,
    tool: &McpToolEntry,
) -> bool {
    match sandbox.mode {
        SandboxMode::ReadOnly => false,        // M7 conservative default
        SandboxMode::WorkspaceWrite => allow_inside_worktree(server, tool),
        SandboxMode::WorkspaceWriteWithNetwork => true,
        SandboxMode::FullAccess => true,
    }
}
```

The exact rules need refinement when M4 (sandbox) ships its proper
tier-3 enforcement. M7's heuristic is conservative: under
`ReadOnly`, no MCP tools appear at all; under `WorkspaceWrite`, only
servers whose name starts with a known-safe prefix (`mcp-` or
`local_`) are exposed. M7 documents this as a placeholder; M4 (or
later) will harden.

### 7.6 Per-server timeouts and response shape

Every `call_tool` is wrapped in `tokio::time::timeout(call_timeout)`.
A timeout returns `McpError::Timeout`, which the routing dispatcher
converts to `ToolResultPayload::Error { message: "MCP call timed
out after Ns" }`. The agent gets the error like any other tool
failure and decides what to do.

`McpToolResult::content` is a `Vec<McpContent>` (text, image, etc.).
M7 supports `Text` and converts non-text content to a stub message
(`"<content type 'image' not yet supported>"`). Image and resource
content are M9+ (when the runtime UI can render them).

---

## 8. Snapshot strategy

Unchanged from M6. `EngineSnapshot` v2 already captures everything
the engine needs to resume a run; the daemon owns *no* run state of
its own beyond in-memory bookkeeping (active/queued sets, broadcast
channels). On daemon restart, the runs are read from disk via the
M5/M6 snapshot machinery and resumed (per `--resume-all`).

No schema bump. M6's v2 reader stays. M7 adds zero `EventPayload`
variants.

---

## 9. Persistence integration

### 9.1 No new event variants

Daemon-level events (`RunAccepted`, `RunFinished`,
`DaemonShuttingDown`) are *not* persisted. They're broadcast over
IPC only. This is intentional: the run's own event log already
records `RunStarted`, `RunCompleted`, `RunAborted` — adding a
`RunAcceptedByDaemon` event would duplicate state the daemon's
in-memory `AdmissionController` already knows.

`McpToolCalled` is also not added. The existing `ToolCalled` event
covers it; the tool name suffices to identify the source.

### 9.2 `ToolCalled` event already carries server origin

The tool name is server-namespaced when MCP exposes it:
`mcp_registry.list_tools_for("playwright")` returns entries where
`tool` is e.g. `browser_navigate`. The session's tool list shows
these names directly (no prefix added — the agent sees a flat tool
list per the MCP convention). On `ToolCalled` event write, we record
the unprefixed name. If a future need arises to disambiguate, M8+
can add an `origin: ToolOrigin` field on `ToolCalled` (the variant is
already `#[non_exhaustive]`).

---

## 10. Concurrency model

### 10.1 Daemon process

Single process. Tokio multi-thread runtime (default workers = num
cpus).

Tasks the daemon spawns:
- **One IPC accept task** — listens on the local socket, spawns a
  per-connection task on each accept.
- **One per-connection task** — reads JSON frames, dispatches each
  to the right handler, writes responses.
- **One per-active-run forward task** — reads `EngineRunEvent`s from
  the engine's broadcast receiver, forwards to the
  `BroadcastRegistry`'s per-run channel.
- **One signal-handler task** — listens for SIGTERM/SIGINT/Ctrl-C
  and triggers shutdown.

Internal contention points:
- `AdmissionController` is a `Mutex<...>`. Held briefly during
  admission decisions.
- `BroadcastRegistry` is a `RwLock<...>`. Read-locked during
  per-event broadcast; write-locked during register/deregister.
- `McpRegistry` uses interior mutability (`tokio::sync::Mutex` per
  server connection state).

No `JoinSet`, no fan-out. The engine's per-run task already exists
from M6; the daemon only adds the IPC plumbing around it.

### 10.2 Engine inside the daemon

Single `Arc<Engine>`. All run tasks already share it (M6 behaviour).
The daemon adds nothing here.

### 10.3 MCP servers (subprocess children)

Each configured server with at least one in-flight call has a child
process spawned via rmcp's `TokioChildProcess`. The child's stdin is
the JSON-RPC request stream; the child's stdout is the response /
notification stream. rmcp owns the framing.

If multiple runs reference the same server name, they share the same
child process. `McpRegistry::call_tool` is concurrent-safe — rmcp's
service has internal serialisation via its `Peer` channel.

### 10.4 Cross-run isolation

MCP servers are *shared* across runs by design (warmth, cost). This
means a `playwright` server holding browser state for run A is the
same browser state for run B. M7 documents this. M9+ may add
per-run isolation (`isolation: PerRun`) on `McpServerRef` if real
workloads need it.

### 10.5 Validation (in `surge-core`)

Per memory `feedback_spec_scope_discipline.md` rule 4 (validation
rules belong in surge-core), the following are enforced at graph
load time:

- **`AgentConfig::mcp_servers` references a defined server.** If a
  stage names `mcp_servers = ["playwright"]` and the run config has
  no `[mcp_servers.playwright]` block, validation fails with
  `McpServerUndeclared { stage, server }`.
- **No empty server name.** `""` rejected.
- **`McpServerRef::call_timeout` ≤ 10 minutes.** Higher values
  warned, not rejected (operator override possible via env var).
- **Stdio command path absolute or pure-name.** Stdio command must
  not contain `..` segments — discourages graph-author smuggling.
  This is a rule placed under the existing `validation::*` umbrella.

These rules apply to graph TOML loaded by *anyone* (engine, editor,
external runner), so they live in `surge-core::validation`.

---

## 11. Threading model

Same baseline as M6. Send everywhere. The rmcp client surface is
Send-friendly (per docs.rs/rmcp, `Service::serve` returns a
`Future + Send`), so no `LocalSet` is needed. This is the major
divergence from the ACP bridge: MCP doesn't have the !Send pain ACP
does, and we benefit accordingly.

---

## 12. CLI integration — full design

### 12.1 `surge daemon` subtree

```
surge daemon start [--detached] [--max-active N] [--shutdown-grace D]
surge daemon stop  [--force]
surge daemon status
surge daemon restart
```

- `start --detached` spawns a child `surge-daemon` process that
  detaches from the controlling terminal (Unix: `setsid` via
  `nix`; Windows: `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`).
  Without `--detached`, runs in foreground (useful for `tmux` /
  `systemd-run` setups).
- `stop` sends a `Shutdown` IPC request, waits for graceful drain.
  `--force` skips the drain (sends SIGKILL after 1 s).
- `status` reads pid/sock files, attempts a `Ping`, prints
  active-run count, queue depth, MCP servers known, uptime.
- `restart` is `stop` + `start --detached`.

### 12.2 `--daemon` flag retrofit

Every existing `surge engine ...` subcommand grows a `--daemon` flag.
With it set, the CLI:
1. Resolves the daemon socket (auto-spawn if needed).
2. Constructs `DaemonEngineFacade::connect(socket)`.
3. Routes the call through it instead of the M6
   `LocalEngineFacade`.

Default is the M6 behaviour: in-process. M7 does not invert the
default — that's an M9 polish decision, after the daemon proves
stable in real use.

### 12.3 `surge engine watch` and `logs` cross-process

- **`watch`**: with `--daemon`, opens a `Subscribe` request to the
  daemon. Without `--daemon`, falls back to log-tail mode (reading
  from disk via `Storage`).
- **`logs`**: always reads from disk. No daemon required (the daemon
  doesn't need to be running for log replay — M6 behaviour).

The transparent split: stream-mode wants the daemon (live events);
replay-mode reads disk (no daemon needed).

### 12.4 Auto-spawn semantics

When `--daemon` is set and no daemon is running, the CLI:
1. Logs "starting daemon …" to stderr.
2. Spawns `surge-daemon --detached` (CLI must know where to find
   the daemon binary — packaged alongside as a sibling binary).
3. Polls the socket until it appears (up to 5 s).
4. Continues with the request.

If the daemon binary is missing, the CLI errors with a clear
"surge-daemon binary not found in PATH or alongside surge".
Documented in the README.

---

## 13. Error handling

### 13.1 Daemon-side

```rust
#[non_exhaustive]
pub enum DaemonError {
    #[error("IPC framing error: {0}")]
    Framing(String),
    #[error("admission queue full ({active}/{max} active, {queued} queued)")]
    AdmissionFull { active: usize, max: usize, queued: usize },
    #[error("run not active: {0}")]
    RunNotActive(RunId),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("engine error: {0}")]
    Engine(EngineError),
    #[error("client disconnected mid-request")]
    ClientGone,
    #[error("shutdown in progress")]
    ShuttingDown,
}
```

Each variant maps to a JSON-RPC `Error { code, message }` response
with a stable `code` (so CLI can react programmatically). The codes
are an `ErrorCode` enum included in the public IPC surface.

### 13.2 MCP-side

```rust
#[non_exhaustive]
pub enum McpError {
    #[error("server '{0}' not configured")]
    ServerNotConfigured(String),
    #[error("server '{server}' failed to start: {source}")]
    StartFailed { server: String, source: String },
    #[error("server '{server}' crashed (exit code {exit_code:?})")]
    ServerCrashed { server: String, exit_code: Option<i32> },
    #[error("server '{server}' tool '{tool}' not found")]
    ToolNotFound { server: String, tool: String },
    #[error("MCP call timed out after {0:?}")]
    Timeout(Duration),
    #[error("rmcp transport error: {0}")]
    Transport(String),
    #[error("rmcp service error: {0}")]
    Service(String),
}
```

`RoutingToolDispatcher` translates each into an appropriate
`ToolResultPayload::Error` so the agent learns "the tool failed" but
not necessarily how.

### 13.3 Client-side fallback when daemon unreachable

If the CLI's `--daemon` connect fails after auto-spawn-and-wait,
report:

```
error: daemon socket /home/user/.surge/daemon/daemon.sock did not
       become readable within 5s. Try `surge daemon status` to
       diagnose.
```

Without `--daemon`, the CLI works exactly as M6 — daemon is fully
optional.

---

## 14. Testing strategy

### 14.1 Unit tests

In `crates/surge-orchestrator/src/engine/`:
- `facade.rs`: `LocalEngineFacade` is a thin wrapper; sanity test
  that delegation works.
- `ipc.rs`: serialisation round-trip for every `DaemonRequest` /
  `DaemonResponse` / `DaemonEvent` variant. Frame parser handles
  partial reads, unicode, very-large `Graph` payloads.
- `tools/routing.rs`: routing decisions match the routing table;
  sandbox-filtered tool list excludes the right entries; tool name
  collisions resolve in favour of engine tools.

In `crates/surge-daemon/src/`:
- `admission.rs`: FIFO queue order; concurrent admit attempts at
  capacity; deregister on completion frees a slot.
- `broadcast.rs`: subscribe/unsubscribe; events delivered to all
  subscribers; deregister cleans up.
- `pidfile.rs`: stale lock detection; PID alive vs dead;
  cross-platform path resolution.

In `crates/surge-mcp/`:
- `connection.rs`: state transitions (Disconnected → Connecting →
  Running → Crashed → Reconnecting); restart-on-crash policy
  honoured.
- `registry.rs`: `list_all_tools` aggregates correctly; per-server
  timeout enforced; concurrent calls to the same server serialised.

Target: ~50 unit tests across all crates.

### 14.2 Integration tests

`crates/surge-daemon/tests/end_to_end_*.rs`:
- Spawn the daemon library inline (no subprocess). Open a real local
  socket. Connect a `DaemonEngineFacade`, run a simple flow, observe
  events stream back, verify the run reaches Terminal.
- Daemon startup with stale PID file → recovers cleanly.
- AdmissionController queue: start `max_active + 2` runs; verify the
  first 8 admit, the next 2 queue, and the queued ones admit as the
  early ones complete.
- Graceful shutdown: start a run, send `Shutdown`, verify the run
  emits `Aborted` and the daemon exits within `shutdown_grace`.

`crates/surge-mcp/tests/`:
- Build a minimal in-process mock MCP server using `rmcp`'s `server`
  feature. The mock declares two tools (`echo`, `crash_now`) and is
  spawned as a binary fixture under `crates/surge-mcp/tests/fixtures/
  mock_mcp_server.rs` for the stdio integration tests.
- Spawn the mock as a `Stdio` transport target. List tools, call
  `echo`, verify the response round-trips.
- `crash_now` causes the mock to exit. The in-flight call resolves
  to `McpError::ServerCrashed`; next call triggers reconnect (when
  `restart_on_crash = true`).
- `restart_on_crash = false`: server stays disconnected after first
  crash; subsequent calls return `McpError::ServerNotRunning`.

`crates/surge-orchestrator/tests/engine_m7_*.rs`:
- End-to-end with a `RoutingToolDispatcher`: agent stage opens a
  session, the tool list includes engine tools + filtered MCP
  tools, the agent calls an MCP tool, the result flows back as a
  ToolResultPayload, the stage completes.

Target: ~15 integration tests.

### 14.3 Manual smoke tests (CLI)

Documented in `crates/surge-daemon/README.md`:
- `surge daemon start; surge daemon status; surge daemon stop`
- `surge engine run flow.toml --daemon` from one shell, `surge
  engine watch <run_id>` from another.
- `surge engine ls --daemon` shows active runs.
- Kill the daemon (`-9`) → restart sees stale PID, recovers.

---

## 15. Acceptance criteria

M7 is shipped when all of the following hold:

1. `surge daemon start` works on Linux, macOS, and Windows.
   Daemon listens on an OS-appropriate local socket / named pipe.
2. `surge daemon stop` shuts the daemon down gracefully; in-flight
   runs receive `RunAborted` and the daemon exits within
   `shutdown_grace`.
3. `surge engine run flow.toml --daemon` from one shell, then
   exiting that shell, leaves the run executing in the daemon.
   `surge engine watch <run_id>` from a different shell streams
   events.
4. `surge engine ls --daemon` lists all currently-active runs with
   their statuses.
5. `surge engine stop <run_id> --daemon` cancels a running daemon
   run from any shell.
6. Daemon hosts ≥ `max_active` (default 8) concurrent runs without
   crashing or interleaving event streams. Excess starts queue and
   admit on completion of earlier ones.
7. `EngineFacade` trait has two implementations (`LocalEngineFacade`,
   `DaemonEngineFacade`), and the existing M6 `surge engine run`
   flow works unchanged when `--daemon` is omitted.
8. `crates/surge-mcp/` ships with at least one working stdio MCP
   server integration tested in CI (using rmcp's "everything"
   example server or a mock equivalent).
9. `RoutingToolDispatcher` returns the correct tool list at session
   open: engine-built-ins first, MCP tools filtered by sandbox and
   the stage's `mcp_servers` allowlist, no name collisions.
10. Validation: a graph that names an undeclared MCP server is
    rejected at load with `McpServerUndeclared`. A graph with
    `mcp_servers = []` works (no MCP tools, no validation errors).
11. MCP server crash mid-call → in-flight `tool_call` resolves to
    `ToolResultPayload::Error { message: "MCP server '...' crashed
    mid-call" }`; next call to the same server triggers reconnect
    when `restart_on_crash = true`.
12. The daemon binary's version is recorded at `~/.surge/daemon/version`
    and reported by `surge daemon status`.
13. Snapshot v2 readers work unchanged. M6 v1→v2 transparent upgrade
    still functions. No new schema version.
14. `crates/surge-daemon/README.md` documents: daemon start/stop,
    troubleshooting (stale lock, daemon won't start, MCP server
    won't connect), where logs land, when to use `--daemon` vs not.
15. `crates/surge-mcp/README.md` documents: how to install the
    rmcp-based servers users commonly want (playwright, github,
    postgres, memory), how to declare them in `RunConfig`, the
    `restart_on_crash` policy, the timeout knob.
16. `docs/03-ROADMAP.md` updated with M7 line and M7 surface.
17. `cargo clippy --workspace -- -D warnings` clean (no new
    warnings introduced).
18. `cargo fmt --all --check` clean.
19. `cargo test --workspace` passes including all M7 unit and
    integration tests.

---

## 16. Open questions / future work

### 16.1 Daemon authentication / multi-user

M7's daemon is single-user. If a developer runs surge from two
different OS users on the same machine, each gets their own daemon
(different `~/.surge`). Cross-user RPC, tokens, capability-based
auth — M9+. Not on the v0.x critical path.

### 16.2 Aging / preemption in `AdmissionController`

If interactive surge uses (one-off `surge run`-style) pile up behind
long batch jobs, FIFO blocks them. M8 may add a priority lane or
aging. M7 documents the limitation; users who hit it can raise
`max_active`.

### 16.3 Hot-reload of MCP server config

Editing `mcp_servers` config and having the daemon notice without
restart is a nice-to-have. M7 reads config at run start; in-flight
runs hold a snapshot. M9+ if real demand surfaces.

### 16.4 MCP `resources` and `prompts`

The MCP spec defines two more capability surfaces beyond `tools`:
`resources` (file-like read-only data) and `prompts` (templated
prompt scaffolding). They don't fit `ToolDispatcher`. M9+ would add
parallel `ResourceProvider` and `PromptProvider` traits and threading
through agent stages.

### 16.5 SSE / HTTP MCP transports

rmcp's `transport-streamable-http-client` covers the SSE / HTTP cases
some MCP servers expose (notably remote-hosted ones). M7 wires only
stdio. M7+ feature-flag adds HTTP. The crate split is already friendly
to it (just add a `McpTransportConfig::Http` variant).

### 16.6 Daemon binary distribution / packaging

`systemd` unit, `launchd` plist, Windows Service — M9+ packaging.
M7's `surge daemon start --detached` is enough for daily-driver use.

### 16.7 Auto-restart of daemon across machine reboots

Tied to §16.6. Solo evening-pace users can `surge daemon start` on
login via shell rc. Production workflows want unit files. M9+.

### 16.8 `--daemon` becoming the default

After M7, `--daemon` is opt-in. Once the daemon is battle-tested
(real users across Linux/macOS/Windows for a few months), M9+ may
flip the default. M7 spec deliberately does not pre-decide this.

---

## 17. Estimate

Solo evening pace, calibrated against M5/M6 history (M1-M6 averaged
~50% over raw days):

| Phase | Work | Days |
|-------|------|------|
| 0 — scaffolding | New crates (`surge-daemon`, `surge-mcp`), workspace updates | 1 |
| 1 — core extensions | `AgentConfig::mcp_servers` field, `McpServerRef`, validation rules, `#[non_exhaustive]` audit | 2 |
| 2 — `EngineFacade` trait + `LocalEngineFacade` | Trait definition, in-process delegation impl, plus `RunSummary`/`RunStatus` types | 2 |
| 3 — `surge-daemon` scaffold + IPC framing + protocol types | Crate skeleton, JSON-RPC line-delimited framing, request/response/event enums | 3 |
| 4 — `AdmissionController` + `BroadcastRegistry` | Both data structures with concurrent tests | 2 |
| 5 — `DaemonEngineFacade` + IPC client | The heavy plumbing — connection setup, request multiplexing, event dispatcher | 3 |
| 6 — CLI `--daemon` retrofit + `surge daemon` subtree | Auto-spawn, daemon start/stop/status/restart, integration with existing `engine` subcommands | 2 |
| 7 — rmcp client wrapper + `McpServerConnection` | rmcp integration, state machine, crash detection | 3 |
| 8 — `McpRegistry` + `RoutingToolDispatcher` | Registry semantics, routing table, dispatcher impl | 3 |
| 9 — sandbox-aware tool exposure | Wiring `RoutingToolDispatcher` into `engine::stage::agent` at session-open time | 1 |
| 10 — integration tests | End-to-end daemon + run, MCP server end-to-end, RoutingDispatcher in real flows | 2 |
| 11 — polish | READMEs (`surge-daemon`, `surge-mcp`), risk doc, ROADMAP update, rustdoc | 1 |
| **Total** | | **25 days** |

**Calibrated:** 25 × 1.5 = **37-38 days** = **~7-8 weeks** at solo
evening pace. Aligns with the earlier "~7-9 weeks" estimate from
M6 spec §22.1.

---

## 18. Phasing — recommended PR cadence

Each PR ships independently testable functionality. Targeting 6 PRs
(matches M6's cadence). Critical path is Phase 1 → 2 (every later
phase depends on `EngineFacade` being defined).

| PR | Phases | Days | What lands |
|----|--------|------|-----------|
| **PR 1 — Foundation** | P0 + P1 + P2 | 5 | Scaffold crates, core extensions, `EngineFacade` trait + `LocalEngineFacade`. Everything still in-process; no behaviour change for users. |
| **PR 2 — Daemon core** | P3 + P4 | 5 | `surge-daemon` scaffold, IPC protocol, AdmissionController, BroadcastRegistry. Daemon binary builds and runs; in-memory tests pass; no CLI integration yet. |
| **PR 3 — Daemon end-to-end** | P5 + P6 | 5 | `DaemonEngineFacade` IPC client, CLI `--daemon` flag, `surge daemon` subtree. End-to-end smoke test passes (`run --daemon`, `watch`, `stop`). MCP not wired yet. |
| **PR 4 — MCP scaffold** | P7 | 3 | `surge-mcp` crate, rmcp wrapper, `McpServerConnection` with state machine. Mock MCP server tests pass. No engine integration yet. |
| **PR 5 — MCP routing** | P8 + P9 | 4 | `McpRegistry`, `RoutingToolDispatcher`, sandbox-aware tool exposure. End-to-end agent stage with real MCP server passes. |
| **PR 6 — Polish** | P10 + P11 | 3 | Integration tests across daemon + MCP, READMEs, risk doc, ROADMAP. |
| **Total** | | **25 days** | |

A user who's only excited about the daemon (and not MCP) sees value
after PR 3 — three weeks calibrated. A user excited about MCP for a
specific agent profile sees value after PR 5.

If life eats the second half of the milestone (PR 4-6), PR 1-3
already delivered functional daemon mode without MCP — useful in
its own right.

---

## 19. Review checklist

Before declaring M7 design complete:

- [ ] All accepted divergences from canonical revision (§23) are
      explicitly listed with rationale.
- [ ] All existing tests still compile (additive changes only).
- [ ] All public enums introduced are `#[non_exhaustive]`
      (`DaemonRequest`, `DaemonResponse`, `DaemonEvent`,
      `GlobalDaemonEvent`, `RunStatus`, `McpTransportConfig`,
      `DaemonError`, `McpError`, `ErrorCode`).
- [ ] Validation rules placed in `surge-core`, not engine
      (per `feedback_spec_scope_discipline.md` rule 4).
- [ ] No half-implementations: every feature is either fully
      specified with all required scaffolding (e.g.,
      `RoutingToolDispatcher` includes the full routing table
      construction) or explicitly deferred in §16 with the milestone
      that owns it.
- [ ] Estimate gives both raw days and calibrated weeks (§17).
- [ ] PR cadence outlined with each PR independently shippable
      (§18).
- [ ] No new snapshot schema version.
- [ ] No new event-payload variants required.
- [ ] All new external dependencies (rmcp, interprocess) are
      justified and pinned to a major version with a fallback
      mention if upstream stagnates.

---

## 20. Risk register

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|-----------|
| rmcp 1.x has a breaking API change before M7 ships | medium | medium | Pin to a minor (`>=1.6, <2.0`); CI builds against the latest minor weekly |
| `interprocess` 2.x has subtle differences from 1.x on Windows named pipes | low | high | Manual smoke tests on a Windows VM during PR 3 review |
| Daemon stdout/stderr fills disk on long-running deployments | medium | medium | Daemon logs to `~/.surge/daemon/logs/daemon.log` with rotation (basic via `tracing-appender`); document the location |
| MCP server child process leaks zombie state on rapid daemon restart | low | low | Daemon's shutdown drains MCP children explicitly (§3.10 step 5) |
| First Windows user hits a CR/LF framing bug on the local socket | medium | medium | Tests cover Windows by running CI on `windows-latest`; framing uses `\n` only and serde_json compact |
| AdmissionController FIFO causes user pain on mixed interactive + batch | low | medium | Documented as "raise `max_active` if you hit this"; M8 owns aging |
| Sandbox interaction at session-open doesn't account for tier-3 enforcement (M4 not shipped yet) | high | low | M7's heuristic is conservative; M4 will harden when it ships; documented in §7.5 |

---

## 21. Definition of Done (summary)

§15 acceptance criteria all check, §19 review checklist all check,
PR cadence delivered through `main`, ROADMAP updated, READMEs in
place, no clippy / fmt / test regressions.

---

## 22. Spec scope discipline self-review

Per memory `feedback_spec_scope_discipline.md`:

**Rule 1 — split if >3 subsystems.**
M7 covers: (1) daemon process, (2) MCP delegation. Two subsystems
under the threshold. Combined OK. The promotion of
`gate_after_each` to M8 (§1.2) keeps M7 honest about this — adding
it would have made it 3 with substantial inter-iteration suspension
work.

**Rule 2 — calibrated estimate stated.**
§17: 25 days raw, 37-38 days calibrated, ~7-8 weeks evening pace.
Both numbers in the spec. The spec ships even if calendar pace
slips by 50%.

**Rule 3 — decide-or-defer.**
Audit:
- `gate_after_each`: deferred fully to M8. M6's validation rejection
  stays in place.
- HTTP MCP transports: deferred fully to M7+. Scaffolding does not
  exist; no `McpTransportConfig::Http` ghost variant.
- Daemon authentication: deferred to M9+. No half-impl; no
  placeholder fields on `DaemonRequest`.
- MCP `resources` / `prompts`: deferred to M9+. No
  `ResourceProvider` ghost trait.
- Aging in `AdmissionController`: deferred to M8. FIFO is the only
  policy.

No half-implementations identified.

**Rule 4 — validation in core.**
`McpServerRef` validation lives in `surge-core::validation`
(undeclared-server check, empty-name, command-path safety). Engine
side validates only run-time-only invariants (e.g., subprocess spawn
failure becomes `McpError::StartFailed`).

**Rule 5 — `#[non_exhaustive]` audit.**
All new public enums marked: `DaemonRequest`, `DaemonResponse`,
`DaemonEvent`, `GlobalDaemonEvent`, `RunStatus`,
`McpTransportConfig`, `DaemonError`, `McpError`, `ErrorCode`. New
public structs (`RunSummary`, `McpServerRef`) also marked
`#[non_exhaustive]` so adding fields later is non-breaking.

---

## 23. Accepted divergences from canonical revision

M7 explicitly preserves the existing surge architecture choices that
diverge from `docs/revision/03-engine.md` and `04-acp-integration.md`:

### 23.1 Single-process daemon hosts many runs

**Revision §03-engine** models the daemon as one OS process per run
(`spawn_daemon` with `setsid` / `DETACHED_PROCESS`). Surge's M5/M6
implementation hosts many runs per daemon process. M7 preserves this.

**Why:**
- The engine is already designed around shared state
  (`Arc<Engine>`, broadcast channels per run inside one process).
  Per-run subprocesses would mean per-run engines, per-run bridges,
  per-run MCP registries — duplicating warmth and cost.
- Single-process is significantly simpler to operate (one PID, one
  socket, one log).
- The trade-off is reduced fault isolation: one bad run can crash
  the daemon. Mitigated by tokio task isolation (a panic in one
  task doesn't kill the runtime) and by snapshots that allow
  resume after daemon crash.

**Risk if revision is later enforced:** rework all daemon plumbing
to spawn subprocesses per run. Not anticipated.

### 23.2 ACP bridge is multi-thread Send

**Revision §04-acp-integration** documents the ACP bridge as a
single-thread `LocalSet` with `!Send` futures. Surge's existing
`AcpBridge` (M3-shipped) bridges to a multi-thread tokio runtime via
its own internal LocalSet — engine-facing methods are Send.

M7 preserves this. The `RoutingToolDispatcher` is Send, the
`McpRegistry` is Send (rmcp helps), and the daemon's task graph runs
on the multi-thread runtime.

### 23.3 No bootstrap pipeline yet

**Revision §03-engine** shows an `advance_bootstrap` lifecycle stage
preceding `advance_pipeline`. Surge's M5-M7 series ships only
`advance_pipeline`; bootstrap (description → roadmap → flow.toml) is
M8. M7 does not introduce bootstrap-related daemon machinery.

### 23.4 Daemon doesn't use revision's `Scheduler` exactly

**Revision §03-engine** describes a `Scheduler` that's a registry of
`RunHandle` plus a `start_run` method that spawns a daemon. Surge's
M7 daemon is itself the scheduler-host — `Engine` plays the role of
revision's `Scheduler::start_run`, and the daemon's
`AdmissionController` plays the role of admission policy. Same
behaviour, different decomposition.

---

## 24. Phasing critical-path notes

### 24.1 Phase 1 must land before everything else

Phase 1 introduces the `AgentConfig::mcp_servers` field and the
`McpServerRef` type. Every later phase (2-11) depends on these
existing in `surge-core`. PR 1 cannot be skipped or reordered.

### 24.2 Phase 5 has the most failure-mode complexity

The IPC client (`DaemonEngineFacade`) is the riskiest piece —
multiplexing requests over a single socket, handling client
disconnect, plumbing per-run subscriptions back through the
broadcast channels, dealing with mid-flight client crashes.
Allocate the most thinking time here (the day budget is 3, but
expect to spike to 4 if subtle bugs surface).

### 24.3 rmcp version pin

Set `rmcp = ">=1.6, <2.0"` in `Cargo.toml`. If a 2.0 ships during
M7's calendar window, evaluate the breaking changes; we don't blindly
upgrade.

### 24.4 Windows-specific testing on PR 3

PR 3 wires the daemon end-to-end. Run a manual smoke test on a
Windows machine before merging — local sockets behave differently
on named pipes and we want to catch surprises early, not at PR 6.

### 24.5 Risk-doc on M5/M6/M7 daemon-binary mixing

Write a short note in `crates/surge-daemon/README.md`: "if you
upgrade `surge` (the CLI) without restarting the daemon, the daemon
may use older event-payload serdes. Always `surge daemon restart`
after upgrades." The version handshake on connect catches major
version mismatches; `surge daemon restart` is the explicit fix.

---

*End of M7 design spec.*
