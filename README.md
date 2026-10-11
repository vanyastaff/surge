# Surge

[![CI](https://github.com/vanyastaff/surge/workflows/CI/badge.svg)](https://github.com/vanyastaff/surge/actions)

> **Local-first orchestration for AFK AI coding workflows.**

Surge is a Rust workspace for running long AI coding work as explicit, event-sourced workflow graphs. A run is not one giant prompt and not a swarm of agents negotiating with each other. A run is a `flow.toml`: typed nodes, declared outcomes, and edges. Agents do the work inside bounded stages; the graph decides where execution goes next.

The experience:

```text
describe an idea → review the documents Surge writes → approve a roadmap it sized itself
   → parallel, verified execution in isolated worktrees → a working result you can inspect
```

You write what you want. Surge drafts a description, a roadmap and a flow, and stops at each one for you to
**approve, ask for changes, or reject**. The planner decides how many stages and milestones the work needs: a small
timer app gets one stage, not a forced MVP → beta → prod ladder. Then each task runs its own flow — spec, implement,
exercise the running app, verify — and "done" is only what an independent verifier certified.

## What works today

A recorded, unattended run from one sentence ("a small command-line pomodoro timer in Rust") to a program that builds,
passes its tests and prints the expected output — with every task and milestone carrying verification evidence — is in
[`docs/recorded-e2e.md`](docs/recorded-e2e.md), including the failures that run found and fixed. It is one project on
one runtime; read the limits there.

## Key Features

- **Human-approved plan before code** — description, roadmap and flow each stop at an approve / edit / reject gate.
- **Roadmap with real structure** — optional release stages, missions with validation contracts, milestones, tasks
  with priority, dependencies and explicit parallel groups; the planner sizes it and justifies the stage count.
- **Verification the implementer cannot write** — a sealed read-only verifier owns `verified`; an **App Tester**
  starts the built program and reports evidence; an optional **cross-vendor verifier** runs on a different runtime;
  flow-load warnings flag unverified, same-runtime and end-only verification.
- **Agent-agnostic via ACP** — Claude Code, Codex, Gemini, Cursor, Copilot, OpenCode, Goose, DeepSeek Harness,
  Ollama. See [ADR-0006](docs/adr/0006-acp-only-transport.md).
- **A main agent can drive Surge** — `surge mcp serve` exposes inbox, run status, ledger, run report and (with
  `--allow-write`) steer / resolve / start-bootstrap over MCP; Surge also supervises MCP servers for its agents.
- **Profiles with identity** — each role has a name, icon, colour and a reasoning-effort floor; the flow diagram
  shows them.
- **Event-sourced runs** — append-only per-run SQLite log; replay, fork-from-here, steer and crash recovery are folds, and
  `surge run trace` exports any run as an OpenTelemetry (OTLP/JSON) trace.
- **Safe by default** — sandbox delegated to each runtime; third-party ticket text is fenced as untrusted data;
  hash-pinned skills; provider rate limits park a run instead of failing it; budgets freeze and resume.
- **Thirteen archetypes** — `feature`, `bug-fix`, `refactor`, `security`, `docs`, `migration`, `performance`,
  `spike`, `linear-3`, `linear-with-review`, `multi-milestone`, `single-task`, and `code-review` for judging an
  existing change. `surge engine run --template <name> --prompt "<what you want>"` runs one directly.
- **Optional desktop app (in development)** — Fleet, Roadmap, Missions, Flow, Inbox, Backlog, Agents, Memory and Settings on real run data.
- **Source-agnostic intake** — CLI, Telegram, UI, GitHub Issues and Linear share one path.
- **One git worktree per run** — managed via `git2`; merged or discarded on terminal outcome.

## Status

Surge is **pre-release software**. What is not there yet: hosted or mobile clients, semantic merge-conflict
resolution, a kanban queue, live OpenTelemetry streaming (traces export after the fact). See
[`docs/competitive-comparison-2026-09-29.md`](docs/competitive-comparison-2026-09-29.md) for a sourced comparison with
Factory, Aperant and Agentlas, including where they are ahead. No head-to-head benchmark exists.

Crates: `surge-core` (graph, profile, roadmap, event, validation types), `surge-acp` (ACP client / bridge / registry),
`surge-orchestrator` (engine, bootstrap, roadmap amendment), `surge-persistence` (SQLite runs, memory, analytics),
`surge-daemon` (local engine host), `surge-process` (protected owner panic boundary),
`surge-cli`, `surge-notify`, `surge-intake`, `surge-telegram`, `surge-mcp`
(supervised MCP client), `surge-git`, `surge-ui` (GPUI desktop app).

## Quick Start

```bash
# Build the CLI and daemon together from this checkout
cargo build --locked --release -p surge-cli -p surge-daemon
./target/release/surge --version
./target/release/surge-daemon --version

# No agent needed; run from this checkout
./target/release/surge engine run examples/flow_terminal_only.toml --watch
```

Install both executables together on `PATH` using the
[installation instructions](docs/getting-started.md#install-a-release-archive).
Then, from your project directory:

```bash
surge init --default
surge bootstrap "a small command-line pomodoro timer in Rust"
```

Full setup, agent configuration, and daemon usage are in [`docs/getting-started.md`](docs/getting-started.md).

Windows archives currently have [runtime capability limitations](docs/getting-started.md#windows-runtime-limitations).
The optional desktop shell is not included in release archives.

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

macOS release binaries statically link OpenSSL 3, distributed under the Apache
License, Version 2.0 included in `LICENSE-APACHE`. OpenSSL is developed by the
[OpenSSL Project](https://github.com/openssl/openssl); its
[license terms](https://github.com/openssl/openssl/blob/openssl-3.6.4/LICENSE.txt)
are included with the archives.
