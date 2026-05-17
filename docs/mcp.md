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

```
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

## Deferred

- Persistent cross-run shared servers (`McpServerRef::isolation = Shared`).
- Run-scoped `surge mcp logs --run-id` (needs the run worktree, which
  `RunSummary` does not yet expose).
- Non-stdio transports (HTTP/SSE).
- A shared daemon supervisor primitive (inbox/cockpit/MCP).
