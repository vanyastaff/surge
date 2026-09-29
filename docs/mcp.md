# MCP server lifecycle

Surge connects to [Model Context Protocol](https://modelcontextprotocol.io)
servers over stdio and exposes their tools to agent stages. This page covers
configuration, the connection lifecycle, the sandbox boundary, and the
`surge mcp` operator surface. Design rationale is in
[ADR-0006](adr/0006-acp-only-transport.md) and
[ADR-0014](adr/0014-mcp-server-lifecycle.md).

## Configuring servers

Add `[[mcp_servers]]` entries to `surge.toml`. The CLI (`surge engine run`,
the daemon ticket launcher) reads them per run and threads them into
`EngineRunConfig::mcp_servers`; per-stage `tool_overrides.mcp_add` selects
which servers a stage actually sees.

```toml
[[mcp_servers]]
name = "filesystem"
transport = { kind = "stdio", command = "npx", args = ["-y", "@modelcontextprotocol/server-filesystem", "/work"] }
allowed_tools = ["read_text_file", "list_directory"]   # omit ⇒ all advertised tools
call_timeout = "60s"                                    # default 60s
restart_on_crash = true                                 # default true
sandbox = "workspace-write"                             # optional per-server override (see Sandbox)
```

| Field | Default | Description |
|-------|---------|-------------|
| `name` | required | Identifier referenced in `tool_overrides.mcp_add` |
| `transport` | required | Only `stdio` is supported |
| `allowed_tools` | all | Per-server tool whitelist |
| `call_timeout` | `60s` | Max time for one `tools/call` and for the handshake |
| `restart_on_crash` | `true` | Reconnect after a transport-class failure |
| `sandbox` | inherit run | Per-server [`SandboxMode`](#sandbox) override |

## Lifecycle

Connections are lazy: the child process spawns on first tool use, not at
config load. State machine (runtime-only — never event-sourced):

```text
Disconnected → Connecting → Running ──(transport-dead)──▶ Crashed
                                 ▲                            │ backoff(attempt)
                                 └──── reconnect ◀────────────┘
                                                              │ attempts > 5
                                                              ▼
                                                         Exhausted
```

- **Crash detection** is structural — `rmcp::ServiceError::{TransportClosed,
  TransportSend}` mark the connection crashed; service-level errors leave it
  alive. (No display-string heuristic.)
- **Restart policy**: exponential backoff (`500ms · 2^(n-1)`, capped at
  `30s`), max **5** consecutive attempts. A call during backoff fast-returns
  without spawning; the no-hot-loop guarantee depends on rate-limited callers.
- **Give-up**: after the cap, the connection becomes `Exhausted`, logs one
  ERROR on `mcp::supervisor`, and the orchestrator appends a replay-safe
  `EscalationRequested` event — which the Telegram cockpit renders as an
  Escalation card. AFK operators see permanent MCP failure.
- **Health monitor**: a per-connection task probes every **60s** (≥ backoff
  cap, so it can't hot-loop) via `is_closed()` then a single-page
  `tools/list`. **3** consecutive transport-class failures mark the
  connection `Unhealthy` and hand it to the restart policy. The monitor is
  bound to the run's cancellation token and exits on run teardown.
- **Teardown**: MCP children are scoped to a single run. On terminal outcome
  the engine calls `McpRegistry::shutdown()` (time-bounded), which
  `cancel().await`s each rmcp service — deterministic, no orphaned children
  (rmcp's `Drop` alone is async best-effort).

## stderr capture & redaction

Child stderr is forwarded to the `mcp::child::stderr` tracing target **and**
appended to a bounded (last 500 lines), run-scoped file
(`<worktree>/.surge/mcp-stderr/<server>.log`; daemon probes use a temp dir).
**Every line is redacted** before it is logged or written — bearer tokens,
`api_key=`/`token=`/`password=` values, and high-entropy blobs are masked.
Redaction is always on in v0.1 (no opt-out knob — a deliberate
decide-or-defer).

## Sandbox

Surge configures *intent*; it does not police MCP-child syscalls — deeper
enforcement is delegated to the agent runtime per
[ADR-0006](adr/0006-acp-only-transport.md). The single canonical resolver
`mcp_spawn_policy` decides exposure from the effective mode (per-server
`sandbox` override, else the run's `SandboxMode`):

| Effective mode | MCP |
|---|---|
| `read-only` | **Denied** — server not spawned, tools hidden |
| `workspace-write` / `workspace-network` / `full-access` / `custom` | Allowed |
| unknown (future tier) | **Denied** (fail-closed) |

Portable hygiene is applied at spawn regardless: the child gets only its
declared `env` plus a minimal essential set (no host-env leak), and its cwd
is pinned to the run worktree. `full-access` / `workspace-network` MCP
children run **unconstrained** — surge emits a one-time WARN; configure only
trusted binaries for those modes.

## `surge mcp`

Request-scoped validation — no persistent daemon-resident sessions
(ADR-0014). Requires a running daemon (`surge daemon start`).

| Command | Effect |
|---|---|
| `surge mcp list [--format json]` | Probe every configured server (spawn → handshake → `tools/list` → tear down); print health + tool count |
| `surge mcp start <name>` | Probe one server to validate its config |
| `surge mcp logs <name> [--tail N]` | Tail the redacted, daemon-probe-scoped captured stderr for `<name>` |
| `surge mcp stop <name>` | **No-op idempotent ack.** Under per-run isolation there is no persistent daemon-held child to stop; to halt MCP activity in a live run, abort the run |

The daemon control socket is owner-only (Unix mode `0600`; Windows named-pipe
DACL restricted to the creating user) — `surge mcp logs` exposes captured
stderr only to the daemon's OS user.

## Surge as an MCP server

Everything above is Surge as an MCP *client* (it spawns servers for its
agents). `surge mcp serve` is the reverse: Surge itself becomes an MCP server
over stdio, so a "main agent" (Claude Code, Codex, any MCP client) can watch
and drive Surge runs.

```bash
claude mcp add surge -- surge mcp serve                 # read-only
claude mcp add surge -- surge mcp serve --allow-write   # + steer / resolve / bootstrap
```

Other clients take the same command in their MCP config:

```json
{ "mcpServers": { "surge": { "command": "surge", "args": ["mcp", "serve"] } } }
```

The server honors `SURGE_HOME` and resolves the project from its working
directory (the enclosing repository). stdout carries the protocol only; logs
go to stderr.

| Tool | Kind | Backed by | Needs daemon |
|---|---|---|---|
| `surge_inbox(include_done?, limit?)` | read | `surge inbox` | no |
| `surge_run_status(run_id)` | read | inbox classifier + `surge resolve` inspect mode | no |
| `surge_ready_tasks(status?, discovered?, run_id?, limit?)` | read | `surge ready` | no |
| `surge_ledger(run_id?, limit?)` | read | `surge ledger` | no |
| `surge_run_report(run_id, format?)` | read | `surge run report` | no |
| `surge_run_trace(run_id)` | read | `surge run trace` | no |
| `surge_memory_search(query, tags?, limit?)` | read | `surge memory search` | no |
| `surge_steer(run_id, message)` | write | `surge steer` | yes |
| `surge_resolve(run_id, expected_node, decision, note?)` | write | `surge resolve` | yes |
| `surge_bootstrap_start(idea)` | write | daemon durable bootstrap (as the desktop app) | yes |

Each result carries a short prose summary plus structured JSON.
**`structuredContent` is the machine-readable form; the text is for humans.**
`surge_run_report`'s `format` (`summary`, the default, or `markdown`) only
changes the text; the full report is always in `structuredContent`.
`surge_ledger` covers all projects, like `surge ledger`; per-project scoping
lands when runs record their origin repo.

**Limits.** `limit` is validated, never clamped: a value outside `1..=5000`
(`1..=50` for `surge_memory_search`, per category) is an `invalid_argument`
error, and the JSON schema carries the same `minimum`/`maximum`. Listings
report `count` (rows returned), `total` (rows matching, before the limit) and
`truncated` (`total > count`); `surge_inbox` reports them under `done` and
omits `done.runs` unless `include_done` is set. `total` saturates at one
million.

**`surge_run_status`** returns `run`, `registry_status` and `pending_input`:
`null` when the run is not blocked, otherwise a value tagged by `kind`:

| `kind` | Extra fields | Can `surge_resolve` answer it? |
|---|---|---|
| `gate` | `node`, `prompt`, `options: [{outcome, label}]` | yes, `decision` is an `outcome` key; `note` optional |
| `tool_call` | `node`, `prompt` | yes, `decision` is the free-form answer text; `note` is rejected |
| `bootstrap_approval` | `node`, `prompt` when known | never: `human_only_gate` |

A storage or parse failure while reading the pending request is reported as
`failed`, never as a `bootstrap_approval`.

**`surge_resolve`** requires `expected_node` (the `pending_input.node` you were
shown) and echoes the accepted decision as
`decision: {kind: "outcome" | "free_form", value}` next to `run_id` and `node`.

## Errors

Failures are MCP tool errors (`isError: true`) whose structured content is
`{"error": {"kind", "message", "data"?}}`. `kind` is a stable code:

| `kind` | Meaning | `data` |
|---|---|---|
| `write_disabled` | mutating tool on a server started without `--allow-write` | |
| `daemon_not_running` | the tool needs the daemon (`surge daemon start`) and none is reachable | |
| `invalid_run_id` | run id malformed, shorter than 6 characters, or matches several runs | |
| `run_not_found` | well-formed run id, no such run | |
| `invalid_argument` | blank or out-of-range argument (empty message/idea/query/decision, `limit` outside its range, `note` on a `tool_call` answer, unknown `status` filter) | |
| `not_awaiting_input` | `surge_resolve` on a run outside the inbox's `needs_input` group | |
| `human_only_gate` | the run waits at a bootstrap approval (description / roadmap / flow) | |
| `stale_gate` | the run is blocked at a different node than `expected_node` | `expected_node`, `current_node` |
| `invalid_decision` | `decision` is not an outcome the gate declares (or it declares none) | `valid_decisions` |
| `rejected` | the daemon or bootstrap supervisor declined a well-formed request (run not active, no supervisor) | |
| `failed` | internal fault: storage, IO, serialization | |

**Safety rules**

- Write tools are always listed but refused unless the server runs with
  `--allow-write`. Every one starts with a single guard that checks the flag
  and returns the audit handle, so none can skip either step.
- Every accepted mutation is logged via `tracing` (target
  `surge::mcp_serve::audit`) with the client name from the MCP `initialize`
  request (self-reported, not authenticated). Steer text and free-form
  answers are logged by length or placeholder only.
- `surge_resolve` answers only a run that is in the inbox's `needs_input`
  group *at call time*, only at the node named by `expected_node`, and only
  with an outcome the pending gate declares (`surge_run_status` lists them).
  It never answers a bootstrap-mode gate: description, roadmap and flow
  approvals stay human decisions (desktop app, Telegram, `surge bootstrap`).
  The policy is one pure function (`authorize_resolution`) with a unit test
  per gate.
- `surge_bootstrap_start` hands the idea to the daemon's durable bootstrap
  supervisor, which stops at each approval gate (a human answers those); the
  returned `planning_run` can be followed with `surge_run_status`.

## Deferred

- Persistent cross-run shared servers (`McpServerRef::isolation = Shared`).
- Run-scoped `surge mcp logs --run-id` (needs the run worktree, which
  `RunSummary` does not yet expose).
- Non-stdio transports (HTTP/SSE).
- A shared daemon supervisor primitive (inbox/cockpit/MCP).
