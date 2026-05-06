# surge · Project Specification

> Graph-based AI coding orchestrator with adaptive flow generation, agent-native sandbox autonomy, and Telegram-driven approvals.

## What is this

`surge` is a local-first desktop tool for orchestrating long-running AI coding tasks through declarative graphs. The user describes a goal in natural language, the system generates a tailored execution flow (linear for trivial tasks, nested loops for large multi-milestone work), and agents execute autonomously with the user approving only at strategic checkpoints — typically via Telegram, so they don't need to sit at the computer.

It is **not** another kanban or task manager. It is **not** a multi-agent swarm. It is a deterministic engine driving typed graphs where each node is an isolated agent session with declared outcomes that route to specific next nodes.

## Why it exists

Author has a full-time job. Existing AI coding tools require constant attention — approving each tool call, switching terminals, manually advancing stages. The goal is **describe → walk away → return to a PR** with the user being pinged only when a real human decision is needed (typically 2–3 times per run).

## Core principles

1. **Engine is dumb, agents are smart.** Routing decisions are declarative (graph edges by outcome). LLM only writes code, never decides "what to do next" — that's the graph's job.
2. **Adaptive complexity.** Trivial task → 3 nodes. Large project → nested milestone/task loops. The Flow Generator picks structure per-task, the user never selects a "tier" or "methodology".
3. **Compose-like configuration.** Agent routing is declared in a friendly `agents.yml` file with named agents and role routes, similar to `docker-compose.yml`. LLMs should be able to read and edit it safely.
4. **Agent-native sandbox autonomy.** surge does not implement its own OS sandbox. It configures and observes the sandbox/permission system already provided by the selected agent runtime, then pings the user only on (a) declared HumanGate nodes, (b) agent permission/elevation requests, (c) terminal events.
5. **Event-sourced, replayable.** Every run is an append-only event log. Time-travel debugging and "fork from here" are free consequences.
6. **Open source, MIT/Apache-2.0.** No paywalls, no telemetry without opt-in, no CLA. DCO sign-off only.

## Document index

### RFCs (read first, in order)

- [`rfcs/0001-overview.md`](rfcs/0001-overview.md) — Vision, scope, non-goals, glossary
- [`rfcs/0002-execution-model.md`](rfcs/0002-execution-model.md) — Event-sourced engine, state machine, run lifecycle
- [`rfcs/0003-graph-model.md`](rfcs/0003-graph-model.md) — Nodes, edges, outcomes, validation rules
- [`rfcs/0004-bootstrap-and-flow-generation.md`](rfcs/0004-bootstrap-and-flow-generation.md) — Three-stage bootstrap, adaptive complexity
- [`rfcs/0005-profiles-and-roles.md`](rfcs/0005-profiles-and-roles.md) — Profile registry, role-as-first-class-node
- [`rfcs/0006-sandbox-and-approvals.md`](rfcs/0006-sandbox-and-approvals.md) — Agent launch/sandbox settings, approvals, hooks, AGENTS.md, trust
- [`rfcs/0007-telegram-bot.md`](rfcs/0007-telegram-bot.md) — Approval UX, inline keyboard, command interface
- [`rfcs/0008-ui-architecture.md`](rfcs/0008-ui-architecture.md) — Editor canvas, runtime view, replay mode

### Architecture (technical specification)

- [`architecture/01-workspace.md`](architecture/01-workspace.md) — Crate layout, dependencies, build targets
- [`architecture/02-data-model.md`](architecture/02-data-model.md) — Core types, TOML schemas, SQLite schema
- [`architecture/03-engine.md`](architecture/03-engine.md) — Executor, scheduler, state machine implementation
- [`architecture/04-acp-integration.md`](architecture/04-acp-integration.md) — ACP bridge, session lifecycle, tool injection
- [`architecture/05-storage.md`](architecture/05-storage.md) — Event log, materialized views, worktree management

### Component specs (implementation level)

- [`components/cli.md`](components/cli.md) — `surge` CLI surface
- [`components/editor.md`](components/editor.md) — egui canvas editor
- [`components/runtime-ui.md`](components/runtime-ui.md) — gpui runtime/replay view
- [`components/telegram-bot.md`](components/telegram-bot.md) — teloxide bot implementation
- [`components/profiles.md`](components/profiles.md) — Bundled profile catalog with full prompts

### Roadmap

- [`ROADMAP.md`](ROADMAP.md) — Milestones from v0.1 to v1.0 with task breakdown

## How to use this spec with Claude

This specification is designed to be consumed by Claude Code (or similar AI coding agent) to implement the project incrementally. Recommended workflow:

1. **Start with RFCs.** Have Claude read `rfcs/0001` through `0008` in order, no implementation yet. This builds shared context.
2. **Implement by milestone.** Use `ROADMAP.md` to pick a milestone. For each milestone, the RFCs and architecture docs together specify what needs to exist; component specs give implementation-level detail.
3. **One crate at a time.** The workspace is structured so each crate can be implemented and tested independently before integration.
4. **Verify via integration tests.** Each milestone has acceptance criteria in the roadmap — implementation is "done" only when those pass.

## Status

This is a specification document, not yet code. Author is solo, working evenings and weekends. Realistic v0.1 ETA: 3–4 months of part-time work.
