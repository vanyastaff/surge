# surge-mcp

MCP (Model Context Protocol) integration for surge. Wraps the
official [`rmcp`](https://docs.rs/rmcp) crate (`>=1.6, <2.0`) with
surge-flavoured config, registry, restart policy, and crash detection.

M7 supports stdio child-process transport only; HTTP/SSE deferred to
M7+ when there's a real driver.

## Status

The crate is fully wired into the surge engine via M7 PR 5 + PR 6:

- `surge_core::mcp_config::McpServerRef` / `McpTransportConfig` —
  serde-able config types.
- `surge_orchestrator::engine::EngineRunConfig::mcp_servers` —
  per-run registry, populated by the caller of
  `Engine::start_run(...)`.
- `Engine` builds an `Arc<McpRegistry>` per run from
  `run_config.mcp_servers` when non-empty.
- `RoutingToolDispatcher` exposes the configured MCP tools to agent
  stages alongside engine built-ins, intersected with the stage's
  `ToolOverride::mcp_add` allowlist + sandbox heuristic +
  per-server `allowed_tools` whitelist.

## Configuring MCP servers via `surge.toml`

Add `[[mcp_servers]]` entries to your `surge.toml`. The CLI
(`surge engine run`, `surge engine run --daemon`, and the daemon-side
ticket launcher used by L1/L2/L3 automation) reads them on every run
and threads them into `EngineRunConfig::mcp_servers`. The engine
then builds the `Arc<McpRegistry>` itself — no programmatic glue
needed.

```toml
[[mcp_servers]]
name = "playwright"
transport = { kind = "stdio", command = "/usr/local/bin/mcp-playwright", args = ["--headless"] }
allowed_tools = ["browser_navigate", "browser_screenshot"]
call_timeout = "120s"
restart_on_crash = false

[[mcp_servers]]
name = "github"
transport = { kind = "stdio", command = "npx", args = ["@github/mcp-server"] }
# allowed_tools omitted → all tools advertised by the server are exposed
# call_timeout defaults to 60s; restart_on_crash defaults to true.
```

Per-stage `tool_overrides.mcp_add` selects which of these servers
each agent stage actually sees — entries not in any allowlist remain
configured but unused (cheap, no child spawned until first call).

## Configuring an MCP server (programmatic)

```rust
use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
use surge_orchestrator::engine::EngineRunConfig;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

let server = McpServerRef::new(
    "playwright".into(),
    McpTransportConfig::stdio(
        PathBuf::from("/usr/local/bin/mcp-playwright"),
        vec![],
        HashMap::new(),
    ),
    Some(vec!["browser_navigate".into(), "browser_screenshot".into()]),
    Duration::from_secs(60),
    true, // restart_on_crash
);

let run_cfg = EngineRunConfig {
    mcp_servers: vec![server],
    ..EngineRunConfig::default()
};

// Pass run_cfg to Engine::start_run(...). The engine builds an
// Arc<McpRegistry> from run_cfg.mcp_servers and threads it to
// agent stages.
```

In your agent stage's `flow.toml`:

```toml
[nodes.research]
kind = "Agent"
profile = "researcher@1.0"

[nodes.research.tool_overrides]
mcp_add = ["playwright"]
```

The agent at `research` will see only `playwright`'s tools (filtered
through its `allowed_tools` whitelist if specified).

## Field reference

`McpServerRef`:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | String | required | Identifier referenced in `tool_overrides.mcp_add` |
| `transport` | `McpTransportConfig` | required | How surge reaches the server |
| `allowed_tools` | `Option<Vec<String>>` | None (all exposed) | Per-server tool whitelist |
| `call_timeout` | Duration | 60s | Max time for a single `tools/call` |
| `restart_on_crash` | bool | true | Re-spawn on child exit |

`McpTransportConfig::Stdio`:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `command` | PathBuf | required | Server binary path or PATH-resolvable name |
| `args` | `Vec<String>` | empty | CLI args |
| `env` | `HashMap<String, String>` | empty | Extra env vars for the child |

## Lifecycle

Connections are constructed in `Disconnected` state. First `call_tool`
or `list_tools` triggers `ensure_connected`:

1. Spawn child via `tokio::process::Command` + `TokioChildProcess`.
2. Run rmcp handshake bounded by `call_timeout`.
3. Transition to `Running`; cache the `Arc<RunningService>`.

On error (transport-like — broken pipe, EOF, channel closed):
- Mark connection `Crashed`.
- Next call: if `restart_on_crash = true`, reconnect; else return
  `McpError::ServerNotRunning`.

On error (service-like — bad params, tool not found, server policy
rejection):
- Connection stays `Running` (server is alive; the call failed).
- Caller gets `McpError::Service`.

The transport-vs-service distinction uses a string-match heuristic on
the rmcp error message — `"connection"`, `"transport"`, `"broken
pipe"`, `"channel closed"`, `"i/o"`, `"unexpected end"`, `"disconnected"`,
`"eof"` mark transport.

## Lifetime — per-run

MCP server child processes are scoped to a single run. M7's
`Engine::start_run` builds an `Arc<McpRegistry>` per run from
`run_config.mcp_servers`. When the run terminates the registry is
dropped and the child processes exit.

This trades startup cost for isolation: a `playwright` server's
browser state for run A is not visible to run B. M9+ may add an
optional shared-server mode (`McpServerRef::isolation = Shared`)
when warmth across runs becomes important.

## Common server installs

| Server | Install |
|--------|---------|
| Playwright | `npm i -g @modelcontextprotocol/server-playwright` |
| GitHub | `npx @github/mcp-server` (no install) |
| Postgres | `npm i -g @modelcontextprotocol/server-postgres` |
| Memory | `npm i -g @modelcontextprotocol/server-memory` |

(Versions and packages move; check
[modelcontextprotocol.io/servers](https://modelcontextprotocol.io/servers).)

## Troubleshooting

**`McpError::StartFailed`**
The child command failed to spawn. Verify the binary is on `PATH` or
use an absolute path in `command`. Check execute permission. On
Windows, ensure the path resolves to a `.exe` if needed.

**`McpError::Timeout`**
The call exceeded `call_timeout`. Either the server is genuinely slow
(raise the timeout) or it deadlocked on a tool implementation bug
(check the server's logs).

**`McpError::ServerCrashed`**
The child process exited mid-call. With `restart_on_crash = true`,
the next call re-spawns. With `false`, you'll see
`McpError::ServerNotRunning` — restart the run.

**`McpError::ServerNotConfigured`**
The agent stage's `mcp_add = ["foo"]` references a server name not in
the run-level `mcp_servers` registry. Either add the server to the
registry or remove it from the stage's allowlist.
