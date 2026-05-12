# Surge

[![CI](https://github.com/vanyastaff/surge/workflows/CI/badge.svg)](https://github.com/vanyastaff/surge/actions)

> **Local-first orchestration for AFK AI coding workflows.**

Surge is a Rust workspace for running long AI coding work as explicit, event-sourced workflow graphs. A run is not one giant prompt and not a swarm of agents negotiating with each other. A run is a `flow.toml`: typed nodes, declared outcomes, and edges. Agents do the work inside bounded stages; the graph decides where execution goes next.

The target experience:

```text
initialize project → describe work → approve roadmap/flow → walk away → return to a PR
```

## Status

Surge is **pre-release software**. Treat it as an active development workspace, not a stable end-user product. Current implementation by crate:

- `surge-core` — graph, profile, event, sandbox, approval, and validation types.
- `surge-acp` — ACP client / pool / bridge, agent registry, discovery, health, mock ACP agent.
- `surge-orchestrator` — legacy spec pipeline plus the newer graph engine.
- `surge-persistence` — SQLite-backed run storage, event logs, views, memory, analytics.
- `surge-daemon` — long-running local engine host over Unix sockets / Windows named pipes.
- `surge-cli` — agents, specs, git worktrees, graph engine, daemon, registry, memory, insights, analytics.
- `surge-notify` — desktop, webhook, Slack, email, and Telegram delivery backends.
- `surge-mcp` — stdio MCP server lifecycle and tool delegation, currently wired at the library / engine level rather than through a user-facing config file.
- `surge-ui` — GPUI desktop shell under development.

## Key Features

- **Agent-agnostic via ACP** — works with any ACP-conformant agent: Claude Code, Codex, Gemini, Cursor, Copilot, OpenCode, and more. See [ADR-0006](docs/adr/0006-acp-only-transport.md) for the rationale.
- **Source-agnostic** — CLI, Telegram, UI, GitHub Issues, and Linear normalize through one intake path.
- **Sandbox-delegated** — surge configures the agent runtime's native sandbox; no custom OS isolation.
- **Declarative `flow.toml` graphs** — closed `NodeKind` enum, typed outcomes, deterministic routing.
- **Event-sourced** — append-only per-run SQLite log; replay, fork-from-here, and crash recovery are folds.
- **Telegram-first approvals** — desktop / email / Slack / webhook fallbacks in `surge-notify`.
- **One git worktree per run** — managed via `git2`; merged or discarded on terminal outcome.

## Quick Start

```bash
# Build the core workspace (excludes the optional GPUI desktop shell)
cargo build --workspace --exclude surge-ui

# Run the smallest possible flow (terminal node only — no agent needed)
cargo run -p surge-cli --bin surge -- engine run examples/flow_terminal_only.toml --watch
```

Full setup, agent configuration, and daemon usage are in [`docs/getting-started.md`](docs/getting-started.md).

## Documentation

| Guide | Description |
|---|---|
| [Getting Started](docs/getting-started.md) | Requirements, build, run examples, agent configuration, smoke tests |
| [CLI](docs/cli.md) | Command surface, execution paths, project context, roadmap amendments |
| [Workflow](docs/workflow.md) | AFK workflow, flow model, intake sources, roadmap amendments, run lifecycle |
| [Architecture](docs/ARCHITECTURE.md) | Positioning, principles, engine, ACP bridge, storage, crate layout |
| [Hooks](docs/hooks.md) | `pre_tool_use` / `post_tool_use` / `on_outcome` / `on_error` lifecycle, matcher, failure modes |
| [Archetypes](docs/archetypes.md) | Bundled `flow.toml` archetypes with mermaid diagrams |
| [Development](docs/development.md) | `cargo` checks, ignored long-running tests, local runtime state |

Crate-level READMEs:

- [`crates/surge-daemon/README.md`](crates/surge-daemon/README.md)
- [`crates/surge-mcp/README.md`](crates/surge-mcp/README.md)
- [`crates/surge-notify/README.md`](crates/surge-notify/README.md)

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
