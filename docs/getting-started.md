[Back to README](../README.md) · [CLI →](cli.md)

# Getting Started

This page walks through installing Surge from source, building the workspace, running the bootstrap path, and configuring an ACP-compatible coding agent.

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

## Initialize a Project

For a fresh repository, create project configuration and stable project context:

```bash
cargo run -p surge-cli --bin surge -- init --default
cargo run -p surge-cli --bin surge -- project describe
```

`surge init --default` writes a validated `surge.toml` with safe onboarding defaults and the best detected ACP agent, falling back to an installable `claude-acp` entry when no agent is found. Run `surge init` without `--default` for the interactive wizard.

`surge project describe` scans high-signal files such as `AGENTS.md`, `README.md`, `Cargo.toml`, `justfile`, formatter/lint config, and git state, then writes `project.md`. In `--author-mode auto` (the default), it uses the Project Context Author ACP profile when the configured runtime is installed and otherwise falls back to deterministic local rendering. This file is separate from `.ai-factory/` agent context: it is the stable project summary captured into new runs at start time. Use `--dry-run` to preview whether it would change, and `--refresh` to rewrite after meaningful project changes.

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

Inspect or detect available ACP agents on the host:

```bash
cargo run -p surge-cli --bin surge -- registry list
cargo run -p surge-cli --bin surge -- registry detect
cargo run -p surge-cli --bin surge -- agent list
```

Then edit `surge.toml`, rerun `surge init`, or use registry commands to add an ACP agent. The annotated [`surge.example.toml`](../surge.example.toml) shows local, `npx`, custom, TCP, MCP-flavored agents, sandbox defaults, worktree defaults, approvals, Telegram env placeholders, and inbox defaults side by side.

## Smoke-Test an Agent

Once an ACP agent is configured, verify it answers a simple ping and a one-shot prompt:

```bash
cargo run -p surge-cli --bin surge -- ping --agent claude
cargo run -p surge-cli --bin surge -- prompt "Summarize this repository" --agent claude
```

## Run the Bootstrap Path

The canonical first useful run is bootstrap:

```bash
cargo run -p surge-cli --bin surge -- bootstrap "add a small health-check command to this project"
```

Bootstrap generates `description.md`, `roadmap.md`, and `flow.toml`, asks for
console approval after each artifact, then starts the generated follow-up graph.
See [Bootstrap](bootstrap.md) for the edit loop, archetypes, and resume path.

## Run the Minimal Agent Graph

`examples/flow_minimal_agent.toml` is the smallest flow that opens an ACP session. Run it once at least one agent is wired up:

```bash
cargo run -p surge-cli --bin surge -- engine run examples/flow_minimal_agent.toml --watch
```

You can also skip bootstrap with a bundled template:

```bash
cargo run -p surge-cli --bin surge -- engine run --template linear-3 --watch
```

## Local State

Local runtime state lives under `~/.surge/`, including run databases and daemon metadata. Project-local state may appear under `.surge/` inside the project. Both directories are safe to delete to start fresh; deleting `~/.surge/` removes all run history.

For setup troubleshooting, run commands with `RUST_LOG=surge=debug` to see agent detection, project-context scan decisions, and skipped optional files.

## See Also

- [CLI](cli.md) — full `surge` command surface and current-to-target mapping
- [Bootstrap](bootstrap.md) — adaptive flow generation from a free-form prompt
- [Workflow](workflow.md) — how a run flows through bootstrap, the engine, and the event log
- [Development](development.md) — running tests, lints, and ignored long-running checks
