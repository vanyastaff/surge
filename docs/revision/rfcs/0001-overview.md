# RFC-0001 · Project Overview

## Vision

A solo developer with a full-time job describes a coding task in natural language. Several minutes later, after approving a brief description and proposed plan via Telegram, walks away. Hours later receives a Telegram notification that a Pull Request is ready. Total time spent: 60–120 seconds across the run.

This is what `vibe-flow` enables: **truly autonomous AI coding pipelines** where the human is the architect-of-architecture (defines templates, sets policies) but not the operator-of-each-decision.

## Why graphs

Existing AI coding tools fall into two camps:

1. **CLI agents** (Claude Code, Codex, Aider) — single conversational thread. User must drive every decision. No structure to enforce "right path" (spec → review → tests → commit).

2. **Multi-agent swarms** (BridgeSwarm, AutoGPT) — agents talk to each other through LLM calls to coordinate. Expensive in tokens, non-deterministic, hard to debug. When something fails, you don't know why.

`vibe-flow` takes a third path: **explicit directed graphs with typed handoffs**. The graph is the workflow. Each node is an isolated agent session with declared outcome ports (e.g., `done`, `blocked`, `escalate`). Edges route outcomes to next nodes. The engine is a deterministic state machine — agents only do work, the graph decides routing.

This gives:

- **Determinism** — same graph + same inputs → predictable execution path
- **Observability** — graph is the source of truth, you can see exactly where you are
- **Composability** — graphs are TOML files, version-controlled, shareable
- **Replayability** — event-sourced runs allow time-travel and forking from any point

## Why adaptive

Hardcoded pipelines (linear `spec → plan → implement → test → review → PR`) are wrong for trivial tasks (overhead) and wrong for large tasks (no decomposition). The user shouldn't have to choose a "complexity tier" — the system should pick.

The **Flow Generator** is a special bootstrap stage that reads the user's task description and produces a graph tailored to scope and archetype:

- Trivial bug fix → 3 nodes linear
- Small feature → 5–7 nodes linear with Review gate
- Medium feature → 1 milestone with inner Task Loop
- Large project → outer Milestone Loop with nested Task Loops
- Refactor → characterize-behavior → tests → constrained refactor → diff-min review
- Spike/exploration → minimal process, skip review

The user never sees these labels. They see the resulting graph and approve it.

## Why Telegram

Approval interface must be **mobile-first** because the entire value proposition is "don't sit at the computer". Telegram bot with inline keyboard buttons covers 90% of approvals (yes/no/edit). Free to host, works on every phone, no app to build, instant push notifications.

For richer interactions (full diff review, prompt editing) — deeplink to desktop app or GitHub PR. Web app inside Telegram is reserved for v2 if user demand justifies the JS/TS frontend stack addition.

## Scope of v1.0

**In scope:**

- Three-stage bootstrap (Description → Roadmap → Flow) with Telegram approvals
- Adaptive flow generation by complexity and archetype
- 7 built-in roles (Spec Author, Architect, Implementer, Test Author, Verifier, Reviewer, PR Composer)
- Event-sourced engine with SQLite persistence
- ACP bridge for agent-agnostic execution (Claude Code / Codex / Gemini CLI)
- Sandbox modes with per-node configuration
- Worktree-per-run isolation
- Visual graph editor (egui) for viewing/editing pipelines
- Runtime view (gpui) with live progress, logs, artifacts
- Replay mode with time-travel scrubber and fork-from-here
- Telegram bot with inline keyboard approvals and slash commands
- 3 default templates (`rust-crate-tdd`, `rust-cli-feature`, `generic-tdd`)
- Local-only operation, no cloud/SaaS dependency

**Explicit non-goals (v1):**

- Multi-user collaboration on the same run (no real-time presence)
- Cloud-hosted runs (everything local via ACP)
- Web app inside Telegram (deeplinks to desktop instead)
- Plugin system for custom node types (`NodeKind` is closed enum)
- Custom DSL for branch predicates (hardcoded predicates only)
- Subscription / billing / monetization
- Cross-machine sync of runs
- Manager/coordinator agents that do LLM-based routing
- Auto-fixing failed runs without human input
- Auto-detection of project type for `vibe init` (per-run flow generation only)

## Glossary

| Term | Definition |
|------|------------|
| **Run** | One execution of a pipeline from start to terminal node. Has unique ID, immutable event log, lives in single git worktree. |
| **Pipeline** | A graph definition (`flow.toml`). May be ephemeral (per-run generated) or saved as template. |
| **Template** | A reusable pipeline blueprint. Lives in `~/.vibe/templates/` or shipped in-box. Used as few-shot example by Flow Generator. |
| **Node** | A vertex in the graph. Has `id`, `NodeKind`, configuration. Cannot be reused across runs (each run gets fresh node instances). |
| **NodeKind** | Closed enum: `Agent`, `HumanGate`, `Branch`, `Terminal`, `Notify`, `Loop`, `Subgraph`. |
| **Profile** / **Role** | A reusable Agent configuration (system prompt, tools, sandbox, outcomes). Lives in `~/.vibe/profiles/`. Examples: `implementer`, `reviewer`, `spec-author`. |
| **Outcome** | A typed result an Agent reports via `report_stage_outcome` tool. Each declared outcome on a node is an output port that maps to an edge. |
| **Edge** | A connection from a node's outcome port to another node's input. Has `EdgeKind`: `Forward`, `Backtrack`, `Escalate`. |
| **HumanGate** | A node type that pauses execution waiting for human decision via Telegram or UI. |
| **Bootstrap** | The pre-pipeline stage where Description, Roadmap, and Flow are generated and approved before main execution. |
| **Flow Generator** | The bootstrap agent that produces the run-specific graph based on user description and roadmap. |
| **Worktree** | Git worktree branch dedicated to a single run. Created at run start, optionally merged at run end. |
| **Event** | Immutable record in the run's event log. Examples: `RunStarted`, `StageEntered`, `OutcomeReported`, `ApprovalRequested`. |
| **Sandbox** | Per-node policy controlling filesystem/network/shell access. Modes: `read-only`, `workspace`, `workspace+network`, `full-access`. |
| **AGENTS.md** | Markdown rules file format (Linux Foundation standard). Loaded into agent context based on scope (global/profile/project/subdir). |
| **ACP** | Agent Client Protocol. Standard interface for invoking AI coding agents (Claude Code, Codex, Gemini). |

## Success criteria for v1.0

The product is "done enough" when:

1. A user can run `vibe init` in an empty Rust project and get a working `pipeline.toml` after answering 0 questions (auto-detection)
2. A user can run `vibe run "build a JSON5 parser library"` and receive a merged PR within 30 minutes with no terminal interaction beyond 3 Telegram taps
3. A failed run can be replayed and forked from the failure point without redoing successful stages
4. A new role can be contributed by creating a single TOML file in `~/.vibe/profiles/` without code changes
5. The graph editor opens an existing `flow.toml` and renders it correctly with live edit/save
6. End-to-end test suite covering all 7 default roles runs in CI on Linux/macOS/Windows
