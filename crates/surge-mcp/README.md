# surge-mcp

MCP (Model Context Protocol) integration for surge. Wraps the
official [`rmcp`](https://docs.rs/rmcp) crate (`>=1.6, <2.0`) with
surge-flavoured config, registry, restart policy, and crash detection.

M7 supports stdio child-process transport only; HTTP/SSE deferred to
M7+ when there's a real driver.

## Configuring an MCP server

In your run-level config (TOML), declare each server with a
`[mcp_servers.<name>]` table. Example:

```toml
[mcp_servers.playwright]
transport = { kind = "stdio", command = "/usr/local/bin/mcp-playwright" }
allowed_tools = ["browser_navigate", "browser_screenshot"]
call_timeout = "60s"
restart_on_crash = true

[mcp_servers.github]
transport = { kind = "stdio", command = "npx", args = ["@github/mcp-server"] }
```

Then in your agent stage (in `flow.toml`):

```toml
[nodes.research]
kind = "Agent"
profile = "researcher@1.0"

[nodes.research.tool_overrides]
mcp_add = ["playwright"]
```

The agent at `research` will see only `playwright`'s tools (filtered
through `allowed_tools` if specified). `github` is configured but
not exposed to this stage.

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
`"eof"` mark transport. False classification is safe (misclassified
service forces an unnecessary reconnect; misclassified transport
means the next call sees the same dead transport).

## Sharing across runs

MCP servers are SHARED across all runs hosted by the same surge engine
instance. A `playwright` server with browser state spawned by run A is
the same server (same browser state) for run B. This is intentional —
re-spawning per run would lose state and cost startup time.

If a use case needs per-run isolation (different browser per run, etc.),
it's a future extension (`McpServerRef::isolation` field, M9+).

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
