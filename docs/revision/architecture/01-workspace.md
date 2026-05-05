# Architecture 01 · Workspace Layout

## Overview

vibe-flow is a Rust workspace with multiple crates. The split is driven by:
- **Layered architecture**: dependencies flow only downward
- **Different stacks**: editor (egui) and runtime (gpui) need separate UI crates
- **Compilation time**: smaller crates rebuild faster
- **Testability**: pure-logic crates can be tested without UI

## Crate structure

```
vibe-flow/
├── Cargo.toml                  (workspace)
├── README.md
├── ARCHITECTURE.md             (link to spec docs)
├── CONTRIBUTING.md
├── LICENSE-MIT
├── LICENSE-APACHE
│
├── crates/
│   ├── core/                   (pure types, no I/O)
│   ├── engine/                 (state machine, executor)
│   ├── storage/                (SQLite + filesystem)
│   ├── acp/                    (ACP integration)
│   ├── sandbox/                (OS-level sandboxing)
│   ├── telegram/               (bot service)
│   ├── editor/                 (egui editor binary)
│   ├── runtime-ui/             (gpui runtime binary)
│   ├── cli/                    (CLI binary)
│   └── testing/                (test utilities)
│
├── profiles/                   (bundled profiles, copied to ~/.vibe/profiles/)
│   ├── _bootstrap/
│   │   ├── description-author-1.0.toml
│   │   ├── roadmap-planner-1.0.toml
│   │   └── flow-generator-1.0.toml
│   ├── spec-author-1.0.toml
│   ├── architect-1.0.toml
│   ├── implementer-1.0.toml
│   ├── test-author-1.0.toml
│   ├── verifier-1.0.toml
│   ├── reviewer-1.0.toml
│   └── pr-composer-1.0.toml
│
├── templates/                  (bundled templates)
│   ├── rust-crate-tdd/
│   │   ├── template.toml
│   │   └── pipeline.toml
│   ├── rust-cli-feature/
│   └── generic-tdd/
│
├── docs/                       (spec docs)
│   ├── README.md
│   ├── rfcs/
│   ├── architecture/
│   └── components/
│
├── tests/                      (workspace-level integration tests)
│   ├── e2e/
│   ├── fixtures/
│   └── snapshots/
│
└── tools/
    ├── profile-validator/      (CLI tool to validate profiles)
    └── event-log-inspector/    (debug tool)
```

## Crate descriptions

### `core` — pure data model

Lowest layer. No I/O, no async, no UI.

**Contents:**
- `NodeKind`, `Node`, `Edge`, `Graph` types (RFC-0003)
- `Event`, `EventPayload` types (RFC-0002)
- `RunState`, fold function (RFC-0002)
- `Profile`, `Role` types (RFC-0005)
- `SandboxMode`, `ApprovalPolicy` types (RFC-0006)
- TOML serialization/deserialization
- Validation logic

**Dependencies:**
```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
toml = "0.8"
toml_edit = "0.22"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v7", "serde"] }
domain-key = { workspace = true }   # author's existing crate
thiserror = "1"

[dev-dependencies]
proptest = "1"
insta = "1"
```

**No async runtime.** No tokio in core.

### `engine` — execution

The state machine that drives runs.

**Contents:**
- Run executor (consumes events, produces events)
- Stage execution logic per `NodeKind`
- Outcome resolution and edge routing
- Hook execution
- Loop iteration management
- Scheduler (multiple concurrent runs)
- Crash recovery

**Dependencies:**
```toml
[dependencies]
core = { path = "../core" }
storage = { path = "../storage" }
acp = { path = "../acp" }
sandbox = { path = "../sandbox" }
tokio = { version = "1", features = ["full"] }
async-trait = "1"
tracing = "0.1"
```

### `storage` — persistence

Wraps SQLite for event log + materialized views, plus filesystem for artifacts.

**Contents:**
- SQLite schema and migrations
- Event log append + read
- Materialized view maintenance
- Artifact storage (filesystem under `~/.vibe/runs/<run_id>/artifacts/`)
- Worktree management (creating, listing, cleaning git worktrees)
- Run directory layout

**Dependencies:**
```toml
[dependencies]
core = { path = "../core" }
sqlx = { version = "0.8", features = ["sqlite", "runtime-tokio"] }
git2 = "0.19"                        # for worktree manipulation
tokio = { version = "1", features = ["fs"] }
```

### `acp` — agent integration

Bridge to ACP (Agent Client Protocol). Handles agent invocation, session lifecycle, tool injection.

**Contents:**
- ACP bridge (dedicated thread + LocalSet pattern, similar to Surge)
- Session pool
- Tool injection (`report_stage_outcome`, sandbox-filtered MCP tools)
- Agent registry (Claude Code, Codex, Gemini executable lookup)
- Streaming support for live tool calls

**Dependencies:**
```toml
[dependencies]
core = { path = "../core" }
agent-client-protocol = "..."        # whatever the official crate is
tokio = { version = "1" }
```

This crate may be either built fresh or extracted from the author's `surge` project's `surge-acp` crate. The author has noted this could be either approach.

### `sandbox` — OS-level enforcement

Per-OS sandboxing implementations.

**Contents:**
- Trait `Sandbox` with `apply_to_command(...)`, `check_path(...)`, etc.
- Linux impl: Landlock (via `landlock` crate), nsjail wrapper
- macOS impl: sandbox-exec wrapper
- Windows impl: AppContainer + Job Objects
- Path checking (always-deny patterns)
- Network allowlist enforcement

**Dependencies:**
```toml
[dependencies]
core = { path = "../core" }
landlock = { version = "0.4", optional = true }    # Linux only

[target.'cfg(target_os = "linux")'.dependencies]
landlock = "0.4"
```

### `telegram` — bot service

Standalone binary running the Telegram bot.

**Contents:**
- teloxide-based bot
- Inline keyboard rendering
- Slash command handling
- Setup flow (binding token)
- Multi-channel support (Slack/email future)

**Dependencies:**
```toml
[dependencies]
core = { path = "../core" }
storage = { path = "../storage" }
teloxide = "0.13"
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

### `editor` — egui editor binary

The visual graph editor.

**Contents:**
- eframe app
- Canvas with egui-snarl for node graph
- Sidebar (project/template/library)
- Inspector (per node-kind tabs)
- TOML import/export

**Dependencies:**
```toml
[dependencies]
core = { path = "../core" }
storage = { path = "../storage" }     # for template registry, project listing
eframe = "0.27"
egui = "0.27"
egui-snarl = "0.4"
```

### `runtime-ui` — gpui runtime binary

The runtime/replay viewer.

**Contents:**
- GPUI app
- Live mode (event-driven updates)
- Replay mode (scrubber, state snapshots)
- Diff viewer
- Cost charts
- Fork-from-here flow

**Dependencies:**
```toml
[dependencies]
core = { path = "../core" }
storage = { path = "../storage" }
engine = { path = "../engine" }       # for fork operation
gpui = "..."
gpui-component = "..."
```

### `cli` — CLI binary

Main entry point for users.

**Contents:**
- `clap`-based command parser
- All commands: `run`, `init`, `list`, `status`, `attach`, `cancel`, `replay`, `fork`, `profile`, `template`, `telegram`, `doctor`
- Daemon spawning logic
- Output formatting (text + JSON)

**Dependencies:**
```toml
[dependencies]
core = { path = "../core" }
engine = { path = "../engine" }
storage = { path = "../storage" }
clap = { version = "4", features = ["derive"] }
tokio = { version = "1" }
indicatif = "0.17"                    # progress bars
console = "0.15"                      # terminal colors
```

### `testing` — test utilities

Shared test helpers, fixtures, mock implementations.

**Contents:**
- Event log fixtures
- Mock ACP agent (deterministic responses)
- Property-based test generators for graphs
- Snapshot test helpers

**Dependencies:**
```toml
[dev-dependencies]
core = { path = "../core" }
proptest = "1"
insta = "1"
```

## Build profiles

```toml
[profile.dev]
opt-level = 0
debug = true
incremental = true

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = true                          # smaller binaries

[profile.release-debug]
inherits = "release"
debug = true
strip = false
```

`release-debug` is for distributing release binaries with symbols for crash reports.

## Binary outputs

Built binaries (debug or release):
- `vibe` — CLI (the main binary users invoke)
- `vibe-editor` — editor GUI
- `vibe-runtime` — runtime GUI
- `vibe-tg` — Telegram bot service (daemon)

CLI can spawn the others when needed (e.g., `vibe replay` opens `vibe-runtime`).

## Dependency rules

Strict layering enforced via Cargo.toml:

- `core` depends on nothing (in our codebase)
- `storage`, `acp`, `sandbox` depend on `core`
- `engine` depends on `core`, `storage`, `acp`, `sandbox`
- `telegram` depends on `core`, `storage`
- `editor` depends on `core`, `storage`
- `runtime-ui` depends on `core`, `storage`, `engine`
- `cli` depends on `core`, `engine`, `storage`

Forbidden:
- `core` depending on anything else internal
- `engine` depending on UI crates
- UI crates depending on each other

`cargo deny` (or similar) configured to enforce these rules in CI.

## CI build matrix

```yaml
strategy:
  matrix:
    os: [ubuntu-latest, macos-latest, windows-latest]
    rust: [stable, nightly]
```

All crates must build on all OS × stable. Nightly informational only.

Skipped per OS:
- Linux Landlock support: only ubuntu (others use no-op sandbox)
- macOS sandbox-exec: only macOS
- Windows Job Objects: only Windows

## Dependency philosophy

Following the author's existing conventions:
- Prefer stable, popular crates
- Avoid niche/experimental dependencies that might disappear
- Carefully evaluate any new dep that adds significant binary size
- Prefer `tokio` over alternatives

Specifically rejected (per Surge experience):
- WASM-based plugins
- Heavy ORMs (sqlx is fine, diesel is too much)

## Workspace Cargo.toml

```toml
[workspace]
members = [
    "crates/core",
    "crates/engine",
    "crates/storage",
    "crates/acp",
    "crates/sandbox",
    "crates/telegram",
    "crates/editor",
    "crates/runtime-ui",
    "crates/cli",
    "crates/testing",
]
resolver = "2"

[workspace.package]
edition = "2021"
rust-version = "1.75"
authors = ["..."]
license = "MIT OR Apache-2.0"
repository = "https://github.com/vanyastaff/vibe-flow"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
chrono = { version = "0.4", features = ["serde"] }
domain-key = { version = "..." }      # author's crate
thiserror = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
# ... other workspace deps

[profile.dev]
opt-level = 0

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = true
```

## File organization within crates

Standard Rust patterns:

```
crates/core/
├── Cargo.toml
├── src/
│   ├── lib.rs              (re-exports + crate root)
│   ├── graph/              (graph model)
│   │   ├── mod.rs
│   │   ├── node.rs
│   │   ├── edge.rs
│   │   └── validation.rs
│   ├── event/              (event types)
│   │   ├── mod.rs
│   │   └── payload.rs
│   ├── state/              (state machine types)
│   ├── profile/            (profile types)
│   └── error.rs
├── tests/
│   ├── graph_validation.rs
│   ├── event_fold.rs
│   └── ...
```

Each major concept gets its own module directory if non-trivial.

## Acceptance criteria

The workspace layout is correctly set up when:

1. `cargo build --workspace` succeeds on Linux, macOS, Windows.
2. `cargo test --workspace` passes all tests.
3. `cargo doc --workspace --no-deps` generates documentation without warnings.
4. `cargo clippy --workspace --all-targets -- -D warnings` clean.
5. `cargo fmt --all -- --check` clean.
6. Building only the editor binary doesn't pull in GPUI dependencies and vice versa.
7. The CLI binary size is < 30MB release, < 100MB debug.
8. Cold build time on a 2024 laptop < 2 minutes for full workspace.
