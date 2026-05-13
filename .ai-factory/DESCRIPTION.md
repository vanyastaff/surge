# Surge

## Overview

Surge is a **local-first meta-orchestrator for AFK AI coding**, written in Rust. It runs long, autonomous coding work as explicit, event-sourced workflow graphs. A run is a `flow.toml`: typed nodes, declared outcomes, and edges. Agents do the work inside bounded stages; the graph decides where execution goes next.

The target experience: `initialize project → describe work → approve roadmap/flow → walk away → return to a PR`. The local daemon owns execution; Telegram and the desktop UI are monitoring and approval surfaces.

Surge is **agent-agnostic via ACP** (any ACP-conformant agent: Claude Code, Codex, Gemini, Cursor, Copilot, OpenCode, ...; see [ADR-0006](../docs/adr/0006-acp-only-transport.md)), **source-agnostic** (CLI, Telegram, UI, GitHub Issues, Linear normalize through a single intake path), and **sandbox-delegated** (no custom OS isolation; the agent runtime enforces sandboxing). Status: **pre-release**.

## Core Features

- **ACP bridge** to any conformant coding agent. The bridge runs on a dedicated OS thread with a single-threaded Tokio runtime (`!Send` futures from the SDK). See [ADR-0006](../docs/adr/0006-acp-only-transport.md) for the rationale behind ACP-only transport.
- **Declarative `flow.toml` graphs** with a closed `NodeKind` enum: `Agent`, `HumanGate`, `Branch`, `Loop`, `Subgraph`, `Notify`, `Terminal`. Routing is graph data, not LLM judgment.
- **Event sourcing** — current state is the fold of an append-only event log. Replay, fork-from-here, and crash recovery are folds, not extra subsystems.
- **Adaptive flow generation** — three-stage bootstrap (Description Author → Roadmap Planner → Flow Generator) picks structure per run; the user reviews and approves.
- **Project initialization and stable context** — `surge init` writes safe onboarding defaults, and `surge project describe` generates `project.md` for run-level `project_context` seeding.
- **Reusable profiles** for agent nodes — system prompt, launch config, sandbox intent, allowed tools, declared outcomes, hooks, approval policy.
- **Sandbox delegation** — agent nodes carry a sandbox intent (`read-only`, `workspace-write`, `workspace+network`, `full-access`); the bridge maps to the runtime's native flags via the matrix documented in [`docs/sandbox-matrix.md`](../docs/sandbox-matrix.md). Elevation lifecycle for mid-run permission requests: [`docs/elevation-runbook.md`](../docs/elevation-runbook.md).
- **Injected tools** — `report_stage_outcome` (dynamic per-node enum) and `request_human_input` are exposed to every session.
- **Multi-channel approvals** — Telegram (primary cockpit via `teloxide`), desktop, email, Slack, webhook. Approvals are first-class events.
- **Tracker intake** — GitHub Issues and Linear via a `TaskSource` trait; tracker is master, surge writes only labels and comments.
- **Git worktree per run** — managed by `git2`. Cleaned up on terminal outcome.
- **Per-run SQLite event log** with WAL mode; triggers prevent UPDATE/DELETE on the events table. Materialized views maintained in the same transaction.
- **MCP server lifecycle** — stdio MCP child processes managed by `surge-mcp` for tool delegation.
- **GPUI desktop shell** under `surge-ui` (in development).

## Tech Stack

- **Language:** Rust 2024 edition (MSRV 1.85)
- **Async runtime:** `tokio`
- **Agent protocol:** `agent-client-protocol` (ACP, with `unstable_session_usage`)
- **Serialization:** `serde`, `serde_json`, `toml`, `toml_edit` (edit-aware writes), `bincode` (binary event payloads)
- **CLI:** `clap` (derive)
- **IDs:** `ulid`
- **Errors:** `thiserror` for library crates, `anyhow` for the CLI binary
- **Logging:** `tracing` + `tracing-subscriber` (env-filter)
- **Time:** `chrono`, `humantime-serde`
- **Hashing / encoding:** `sha2`, `hex`
- **Database:** `rusqlite` (bundled SQLite) + `r2d2` / `r2d2_sqlite` connection pool, file lock via `fd-lock`
- **Git:** `git2` (worktree and branch lifecycle)
- **HTTP:** `reqwest` (rustls-tls)
- **Tracker SDKs:** `octocrab` (GitHub), `lineark-sdk` (Linear)
- **Notifications:** `notify-rust` (desktop), `lettre` (SMTP), `tiny_http` (webhook receivers), `owo-colors` (terminal output)
- **MCP:** `rmcp` (client + child-process transport)
- **IPC:** `interprocess` (Unix sockets / Windows named pipes), `nix` (signals)
- **Graph:** `petgraph`
- **Versioning:** `semver`
- **Streams / futures:** `async-stream`, `tokio-stream`, `futures`, `async-trait`
- **Testing:** `proptest`, `insta` (snapshots), `criterion` (benchmarks), `wiremock`, `tokio-test`, `tempfile`
- **System inspection:** `sysinfo`
- **Secrets:** `regex` for secret scanning
- **RNG:** `rand`

## Architecture

See `.ai-factory/ARCHITECTURE.md` for the AI-context architecture guidelines (pattern, folder structure, dependency rules, code examples). The canonical product/architecture document is `docs/ARCHITECTURE.md`.

- **Pattern:** Modular Monolith (Rust Workspace) with Hexagonal Influences

## Architecture Notes

- **Workspace with 12 crates.** Dependencies flow downward; no cycles. `surge-core` is leaf (no I/O). Binaries (`surge-cli`, `surge-daemon`, `surge-ui`) consume the workspace through stable trait surfaces. See `docs/ARCHITECTURE.md` for full layering and the canonical architecture document.
- **Engine is dumb, agents are smart.** Routing decisions are graph data (declarative edges keyed by outcome). The LLM only does the work.
- **Sandbox is delegated.** Surge configures the agent runtime's native sandbox and observes elevation requests; it does not reimplement OS isolation.
- **Closed `NodeKind` enum.** Extensibility happens via profiles, named agents, and templates — not new node kinds.
- **Append-only event log per run.** SQLite with WAL mode; triggers prevent UPDATE/DELETE on `events`. Folding is deterministic — no wall-clock dependencies, no random IDs introduced during fold.
- **Tracker is master.** For tracker-sourced work, surge writes only labels (`surge-priority/<level>`, `surge:enabled`, `surge:auto`, `surge:template/<name>`) and comments; ticket status stays under user control.
- **Single-user, single-machine.** Multi-user collaboration on the same run is a non-goal.

## Crate Layout

| Crate | Responsibility |
|---|---|
| `surge-core` | Graph, profile, event, sandbox, approval, validation types. No I/O. |
| `surge-acp` | ACP bridge, agent pool, agent registry, discovery, health, mock agent. |
| `surge-orchestrator` | Graph executor (`engine/`), bootstrap driver, stable project-context generation/seeding, roadmap-amendment surfaces. |
| `surge-persistence` | SQLite stores, event log, materialized views, memory, analytics. |
| `surge-git` | Worktree and branch lifecycle. |
| `surge-intake` | Issue-tracker sources (`TaskSource` trait + Linear / GitHub Issues impls). |
| `surge-daemon` | Long-running local engine host over Unix sockets / Windows named pipes. |
| `surge-cli` | `surge` binary: init, project context, agents, specs, worktrees, engine, daemon, registry, memory, analytics. |
| `surge-notify` | Notification delivery: desktop, webhook, Slack, email, Telegram. |
| `surge-mcp` | stdio MCP server lifecycle and tool delegation. |
| `surge-ui` | GPUI desktop shell. |

## Non-Functional Requirements

- **Local-first.** Long-running daemon on the user's machine. Cloud deployments are not the primary form factor.
- **Open source.** Dual-licensed MIT / Apache-2.0. No telemetry, no CLA.
- **Observability:** structured logs via `tracing`; per-run event log doubles as the audit trail.
- **Error handling:** `thiserror` for library crates, `anyhow` only at the CLI binary boundary. No `unwrap()` in library code.
- **Concurrency model:** the daemon is the single writer to the event log; CLI / UI / bot are readers. WAL mode lets readers proceed without blocking.
- **Schema versioning:** `schema_version` field per event with a migration chain for old payloads.
- **Determinism:** event folding has no wall-clock or random-ID dependencies — replay produces identical state.
- **Cross-platform:** Windows (named pipes), Linux/macOS (Unix sockets) both supported.
- **Code quality:** strict `clippy.toml` profile (cognitive-complexity ≤ 25, excessive-nesting ≤ 5, function length ≤ 100 lines, struct/enum/fn-arg bool caps); `rustfmt.toml` enforced; relaxed lint rules only inside `#[cfg(test)]`.
