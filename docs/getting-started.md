[Back to README](../README.md) · [CLI →](cli.md)

# Getting Started

This page walks through installing Surge from source, building the workspace, running the smallest possible flow, and configuring an ACP-compatible coding agent.

## Requirements

- Rust `1.85+`
- Git
- An ACP-compatible agent on `PATH` for any flow that contains an `Agent` node (Claude Code, Codex, Gemini, or a custom ACP-conformant binary)

## Build

Build the core workspace (excludes the optional GPUI desktop shell):

```bash
cargo build --workspace --exclude surge-ui
```

The desktop UI is optional and has separate GPUI dependencies:

```bash
cargo build -p surge-ui
```

## Run the Smallest Flow

`examples/flow_terminal_only.toml` contains only a terminal node, so it does not need an agent. This is the canonical smoke test:

```bash
cargo run -p surge-cli --bin surge -- engine run examples/flow_terminal_only.toml --watch
```

Run the same flow through the daemon — start the daemon, run the flow against it, list runs, then stop the daemon:

```bash
cargo run -p surge-cli --bin surge -- daemon start --detached
cargo run -p surge-cli --bin surge -- engine run examples/flow_terminal_only.toml --daemon --watch
cargo run -p surge-cli --bin surge -- engine ls --daemon
cargo run -p surge-cli --bin surge -- daemon stop
```

## Configure Agents in a Project

Initialize project-level configuration, then inspect or detect available ACP agents on the host:

```bash
cargo run -p surge-cli --bin surge -- init
cargo run -p surge-cli --bin surge -- registry list
cargo run -p surge-cli --bin surge -- registry detect
cargo run -p surge-cli --bin surge -- agent list
```

Then edit `surge.toml` or use registry commands to add an ACP agent. The annotated [`surge.example.toml`](../surge.example.toml) shows local, `npx`, custom, TCP, and MCP-flavored agent entries side by side.

## Smoke-Test an Agent

Once an ACP agent is configured, verify it answers a simple ping and a one-shot prompt:

```bash
cargo run -p surge-cli --bin surge -- ping --agent claude
cargo run -p surge-cli --bin surge -- prompt "Summarize this repository" --agent claude
```

## Run the Minimal Agent Graph

`examples/flow_minimal_agent.toml` is the smallest flow that opens an ACP session. Run it once at least one agent is wired up:

```bash
cargo run -p surge-cli --bin surge -- engine run examples/flow_minimal_agent.toml --watch
```

## Local State

Local runtime state lives under `~/.surge/`, including run databases and daemon metadata. Project-local state may appear under `.surge/` inside the project. Both directories are safe to delete to start fresh; deleting `~/.surge/` removes all run history.

## See Also

- [CLI](cli.md) — full `surge` command surface and current-to-target mapping
- [Workflow](workflow.md) — how a run flows through bootstrap, the engine, and the event log
- [Development](development.md) — running tests, lints, and ignored long-running checks
