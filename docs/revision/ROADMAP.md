# ROADMAP

## Overview

This roadmap breaks the implementation into **milestones with concrete tasks**. Each milestone is independently shippable — at the end of each, you have a working slice of vibe-flow.

The roadmap is **realistic for solo evening/weekend work** — author has full-time job, builds Nebula and Surge in parallel. Estimates assume 8-12 hours/week of focused vibe-flow work.

## Realistic timeline

- **v0.1 MVP** — 12-14 weeks (3-3.5 months)
- **v0.2** — additional 8-10 weeks
- **v0.3** — additional 6-8 weeks
- **v0.4** — additional 6-8 weeks
- **v1.0** — additional 4-6 weeks (polish, audit, release prep)

Total: 9-12 months from start to v1.0. Don't try to compress this.

---

## v0.1 MVP

**Goal**: A user can run `vibe run "<description>"` from a Rust project, get bootstrapped through Telegram approvals, and end up with a merged PR — without ever opening a desktop UI.

**Includes**: CLI, engine, ACP integration, sandbox, Telegram bot, 7 standard profiles, 3 templates.

**Excludes**: Visual editor, runtime UI, replay/fork features.

### M0: Foundation (week 1-2)

Set up the workspace and shared crates. Nothing user-visible yet.

**Tasks:**
- T0.1 — Create workspace with all 10 crates as empty stubs (`crates/core`, `crates/engine`, etc.). Verify `cargo build --workspace` succeeds.
- T0.2 — Set up CI (GitHub Actions): build matrix Linux/macOS/Windows × stable. Run `cargo test`, `cargo clippy`, `cargo fmt`.
- T0.3 — Configure `cargo deny` for license + dependency rules (per architecture/01).
- T0.4 — Add LICENSE-MIT, LICENSE-APACHE, CONTRIBUTING.md, README.md, ARCHITECTURE.md (link to spec).
- T0.5 — Set up `tracing` + `tracing-subscriber` workspace-wide for structured logging.
- T0.6 — Add error type derivation pattern (workspace-wide use of `thiserror::Error`).
- T0.7 — Set up `insta` snapshot testing infrastructure.

**Acceptance**: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace -- -D warnings` clean on all 3 OSes in CI.

### M1: Core types and serialization (week 3-4)

Implement the data model from architecture/02-data-model.md.

**Tasks:**
- T1.1 — Implement `core/graph.rs`: `Graph`, `Node`, `NodeKind` (closed enum), `Edge`, all variants of `NodeConfig`.
- T1.2 — Implement `core/event.rs`: `Event`, `EventPayload` enum with all 30+ variants.
- T1.3 — Implement `core/profile.rs`: `Profile`, `Role`, etc.
- T1.4 — Implement `core/state.rs`: `RunState`, `RunMemory`, fold function.
- T1.5 — TOML serialization round-trip for `Graph` (using `serde` + `toml` for read, `toml_edit` for write).
- T1.6 — TOML serialization round-trip for `Profile`.
- T1.7 — bincode serialization for `EventPayload`.
- T1.8 — Implement graph validation (15 rules from RFC-0003).
- T1.9 — Property-based tests using proptest: random graphs, fold, validation.
- T1.10 — Snapshot tests for handcrafted fixtures.

**Acceptance**: All RFC-0003 acceptance criteria pass. Test suite has >80% coverage on core crate.

### M2: Storage layer (week 5-6)

Implement the storage from architecture/05-storage.md.

**Tasks:**
- T2.1 — SQLite migrations setup (sqlx-cli or custom runner).
- T2.2 — Registry DB schema (runs, profiles, templates, trust).
- T2.3 — Per-run DB schema (events, materialized views).
- T2.4 — Implement `Storage::create_run`, `open_run`, `list_runs`, `delete_run`.
- T2.5 — Implement `RunHandle::append_event`, `read_events`, `current_seq`.
- T2.6 — Implement materialized view maintenance (engine-side).
- T2.7 — Implement materialized view rebuild from events.
- T2.8 — Profile and template registry: install, list, load, validate.
- T2.9 — Artifact storage on filesystem with content-addressed IDs.
- T2.10 — Worktree management: create, cleanup, merge (via `git2` or shell git).
- T2.11 — Live event subscription via polling (200ms interval).
- T2.12 — Crash-safety tests: SIGKILL during writes, verify recovery.

**Acceptance**: All RFC-0002 and architecture/05 acceptance criteria pass. WAL mode works. Concurrent reads during writes don't block.

### M3: ACP integration (week 7-8)

Implement the ACP bridge from architecture/04-acp-integration.md.

**Tasks:**
- T3.1 — Either extract bridge pattern from author's `surge-acp` or implement from scratch.
- T3.2 — Bridge thread with `LocalSet` for `!Send` futures.
- T3.3 — `BridgeCommand` channel + `BridgeEvent` broadcast.
- T3.4 — `AcpBridge::open_session`, `send_message`, `close_session`.
- T3.5 — Session observer: emits `BridgeEvent` for tool calls, agent messages.
- T3.6 — Agent registry: detect Claude Code, Codex, Gemini in PATH.
- T3.7 — Tool injection: `report_stage_outcome` with dynamic outcome enum.
- T3.8 — Tool injection: `request_human_input`.
- T3.9 — Sandbox-filtered MCP tool list per session.
- T3.10 — Session crash detection (subprocess exit) → `SessionEnded` event.
- T3.11 — Token usage tracking from agent messages.
- T3.12 — Mock ACP agent for testing (in `crates/testing`).

**Acceptance**: Can open a session with at least Claude Code (or mock), exchange messages, observe tool calls, close cleanly. Multiple concurrent sessions work.

### M4: Sandbox (week 9)

Implement sandbox enforcement from RFC-0006.

**Tasks:**
- T4.1 — Define `Sandbox` trait + 4 modes (`read-only`, `workspace-write`, `workspace+network`, `full-access`).
- T4.2 — Tier 1 enforcement: MCP tool filtering.
- T4.3 — Tier 2 enforcement: filesystem path checking.
- T4.4 — Tier 3 Linux: Landlock integration via `landlock` crate.
- T4.5 — Tier 3 macOS: `sandbox-exec` wrapper.
- T4.6 — Tier 3 Windows: AppContainer + Job Objects (most limited).
- T4.7 — Tier 4 network: domain allowlist for outbound HTTPS.
- T4.8 — Sandbox elevation flow: detect, request, decide, persist if "remember".
- T4.9 — Always-deny patterns (`.git`, `.vibe`, secrets).
- T4.10 — `vibe doctor` reports sandbox capability per OS.

**Acceptance**: Attempted escapes (write outside worktree, access ~/.ssh, network to non-allowlisted) are blocked at appropriate tier on each OS.

### M5: Engine executor (week 10-11)

Implement the engine from architecture/03-engine.md.

**Tasks:**
- T5.1 — `Executor` struct with run lifecycle methods.
- T5.2 — Per-NodeKind execution: Agent (most complex), HumanGate, Branch, Terminal, Notify.
- T5.3 — Bootstrap orchestration (3 stages with approval gates).
- T5.4 — Outcome routing: outcome → edge → next node.
- T5.5 — Hook execution: pre_tool_use, post_tool_use, on_outcome, on_error.
- T5.6 — Retry logic: stage failure → re-enter with attempt+1.
- T5.7 — Loop execution: iterate body subgraph over collection.
- T5.8 — Subgraph execution.
- T5.9 — Crash recovery: scan non-terminal runs, fold state, decide recovery action.
- T5.10 — Daemon process: spawn detached subprocess (Linux/macOS via `setsid`, Windows via `DETACHED_PROCESS`).
- T5.11 — Scheduler: track multiple concurrent runs.
- T5.12 — End-to-end test: handcrafted 5-node graph executes correctly using mock agent.

**Acceptance**: All RFC-0002 and architecture/03 acceptance criteria pass.

### M6: CLI (week 12)

Implement CLI commands from components/cli.md.

**Tasks:**
- T6.1 — `clap`-based command parser with all subcommands.
- T6.2 — Implement: `run`, `init`, `list`, `status`, `attach`, `cancel`, `replay` (without UI).
- T6.3 — Implement: `profile list/show/install/uninstall/validate/diff`.
- T6.4 — Implement: `template list/show/install/uninstall/validate`.
- T6.5 — Implement: `telegram setup/test/unbind/start`.
- T6.6 — Implement: `doctor`, `gc`, `fork`.
- T6.7 — JSON output mode for all commands.
- T6.8 — Shell completion generation (bash, zsh, fish).
- T6.9 — Daemon spawning logic (cross-platform).
- T6.10 — Live attach: tail event log, format events for terminal.

**Acceptance**: All commands work in text and JSON modes. `vibe doctor` correctly diagnoses common issues.

### M7: Telegram bot (week 13)

Implement Telegram bot from components/telegram-bot.md.

**Tasks:**
- T7.1 — `teloxide` setup, bot binary `vibe-tg`.
- T7.2 — Long-poll mode, dispatcher.
- T7.3 — Setup flow: ephemeral binding token, /start handling.
- T7.4 — Outgoing pipeline: poll for `pending_approvals`, build cards, send.
- T7.5 — Card builders for each approval type (Description, Roadmap, Flow, HumanGate, Sandbox, Progress, Completion, Failure).
- T7.6 — Inline keyboard generation with callback_data.
- T7.7 — Callback query handler: parse data, write `ApprovalDecided` event, edit message.
- T7.8 — Slash commands: `/run`, `/list`, `/status`, `/cancel`, `/replay`, `/help`.
- T7.9 — Free-text reply handler for "Edit" feedback.
- T7.10 — Secrets filtering with regex patterns.
- T7.11 — Rate limiting (token bucket per chat + global).
- T7.12 — Webhook mode (optional).

**Acceptance**: All components/telegram-bot.md acceptance criteria pass. End-to-end: full bootstrap (3 approvals) via Telegram works.

### M8: Profile catalog and templates (week 14)

Implement the 10 v1 profiles + 3 templates.

**Tasks:**
- T8.1 — Write `_bootstrap/description-author-1.0.toml` with full prompt.
- T8.2 — Write `_bootstrap/roadmap-planner-1.0.toml`.
- T8.3 — Write `_bootstrap/flow-generator-1.0.toml`.
- T8.4 — Write `spec-author-1.0.toml`.
- T8.5 — Write `architect-1.0.toml`.
- T8.6 — Write `implementer-1.0.toml`.
- T8.7 — Write `test-author-1.0.toml`.
- T8.8 — Write `verifier-1.0.toml`.
- T8.9 — Write `reviewer-1.0.toml`.
- T8.10 — Write `pr-composer-1.0.toml`.
- T8.11 — Create `rust-crate-tdd` template (TOML + pipeline.toml + readme).
- T8.12 — Create `rust-cli-feature` template.
- T8.13 — Create `generic-tdd` template.
- T8.14 — Test fixtures for each profile (3+ scenarios per profile).
- T8.15 — Integration test: full pipeline using all 7 standard profiles against a real test crate.

**Acceptance**: All profiles validate, load. Flow Generator successfully builds graphs using these profiles for 10+ test descriptions covering different complexities and archetypes. Real end-to-end run on test crate succeeds.

### v0.1 release criteria

The v0.1 milestone is complete when:

1. A new user can install vibe-flow on Linux/macOS/Windows (binary distribution or `cargo install`)
2. They can run `vibe telegram setup` and bind their bot
3. They can run `vibe run "<description>"` from a Rust project
4. They receive Telegram bootstrap cards (Description → Roadmap → Flow)
5. After approving, the run executes autonomously
6. They receive a final card with PR link
7. The PR is mergeable and contains coherent code
8. End-to-end takes <30 minutes for a small Rust crate task
9. CI passes on all 3 OSes
10. Documentation is complete enough for a user to get started without asking the author questions

**No visual UI required for v0.1.** CLI + Telegram is enough.

---

## v0.2 — Visual editor + runtime view

**Goal**: Users can visualize and edit pipelines, monitor runs in a live UI.

### M9: Editor MVP (week 15-18)

**Tasks:**
- T9.1 — eframe app skeleton with top bar, sidebar, central canvas, inspector.
- T9.2 — Integrate egui-snarl for node graph rendering.
- T9.3 — Custom node rendering per NodeKind (icons, colors, ports).
- T9.4 — TOML load → render graph correctly.
- T9.5 — Inspector tabs for Agent node (most complex): General, Context, Prompt, Tools, Sandbox, Approvals, Hooks, Outcomes, Advanced.
- T9.6 — Inspector tabs for HumanGate, Branch, Terminal, Notify.
- T9.7 — Edge creation by drag-drop with live validation.
- T9.8 — Save back to TOML using `toml_edit` (preserves comments).
- T9.9 — File menu: Open, Save, Save As, Recent.
- T9.10 — Sidebar: project list, template browser, node library.
- T9.11 — Drag-drop node from library to canvas.
- T9.12 — Loop body editing: collapse/expand subgraph navigation.
- T9.13 — Undo/redo (in-memory history).
- T9.14 — Cross-platform binaries (Linux, macOS, Windows).

**Acceptance**: Can open any v0.1-generated `flow.toml`, view it correctly, edit (add nodes, connect edges, change config), save back, and the modified flow runs correctly.

### M10: Runtime UI MVP — Live mode (week 19-22)

**Tasks:**
- T10.1 — gpui app skeleton with layout from components/runtime-ui.md.
- T10.2 — Theme tokens, font setup.
- T10.3 — Top bar with mode tabs and status chip.
- T10.4 — Mode bar with stats.
- T10.5 — Events panel (left) with live tailing.
- T10.6 — Graph rendering (read-only, custom in gpui — no egui-snarl available).
- T10.7 — Active node animation (pulse, color states).
- T10.8 — Detail panel (right) with active stage card.
- T10.9 — Bottom panel with Events tab (live).
- T10.10 — Live update mechanism: polling + reactive re-render.
- T10.11 — Multiple runs sidebar.
- T10.12 — Cross-platform binaries.

**Acceptance**: Can attach to a running run via `vibe-runtime --run <id>`, see live progress, follow events, view active stage details.

---

## v0.3 — Replay and fork

**Goal**: Power users can debug runs by replaying and fork from any point.

### M11: Replay mode (week 23-26)

**Tasks:**
- T11.1 — Replay mode in runtime UI: time-travel scrubber.
- T11.2 — State snapshot writing during runs (every N events).
- T11.3 — State reconstruction at any seq via snapshot + replay.
- T11.4 — Cursor-based event highlighting in events panel.
- T11.5 — Graph state at cursor (completed/active/future visual states).
- T11.6 — Bottom panel: Diff tab (unified + split views).
- T11.7 — Bottom panel: Artifacts tab with markdown rendering.
- T11.8 — Bottom panel: Cost tab with charts (custom gpui rendering).
- T11.9 — Play/pause animation through replay.
- T11.10 — Keyboard shortcuts for scrubbing.

### M12: Fork-from-here (week 27-28)

**Tasks:**
- T12.1 — Fork dialog with optional adjustments.
- T12.2 — Engine: `fork_run(source, at_seq)` implementation.
- T12.3 — Event log copy + worktree branch creation.
- T12.4 — Optional pre-fork edits: prompt override, profile change.
- T12.5 — New run starts in live mode.

### M13: Compare two runs (week 29-30)

**Tasks:**
- T13.1 — Side-by-side run comparison UI.
- T13.2 — Divergence detection.
- T13.3 — Side-by-side artifact diffs.

---

## v0.4 — Quality and polish

### M14: Hooks ecosystem and AGENTS.md (week 31-33)

**Tasks:**
- T14.1 — Hook execution refinements: timeouts, env injection.
- T14.2 — AGENTS.md JIT loading with token budget management.
- T14.3 — Trust state management UI in editor.
- T14.4 — Project-level `.vibe/` directory full support.

### M15: Specialized profiles (week 34-35)

**Tasks:**
- T15.1 — `bug-fix-implementer@1.0`
- T15.2 — `refactor-implementer@1.0`
- T15.3 — `security-reviewer@1.0`
- T15.4 — `migration-implementer@1.0`
- T15.5 — Templates using these profiles.

### M16: Branch predicates and complex routing (week 36)

**Tasks:**
- T16.1 — All `Predicate` variants (FileExists, ArtifactSize, etc.).
- T16.2 — Predicate builder UI in editor.
- T16.3 — Complex Branch examples in templates.

### M17: Quality of life (week 37-38)

**Tasks:**
- T17.1 — `vibe gc` polish.
- T17.2 — Run export/import (`vibe export <id>`).
- T17.3 — Better error messages across all commands.
- T17.4 — Logging improvements with structured fields.
- T17.5 — Performance audit on long runs (10K+ events).

---

## v1.0 — Release prep

### M18: Documentation (week 39-40)

**Tasks:**
- T18.1 — User guide (getting started, common workflows).
- T18.2 — Reference docs auto-generated from rustdoc.
- T18.3 — Profile authoring guide.
- T18.4 — Template authoring guide.
- T18.5 — Troubleshooting guide.
- T18.6 — Architecture overview for contributors.
- T18.7 — Video demo of full run.

### M19: Distribution (week 41-42)

**Tasks:**
- T19.1 — Binary builds for major platforms (release artifacts).
- T19.2 — Homebrew formula.
- T19.3 — `cargo install vibe-flow` works.
- T19.4 — AUR package (Arch).
- T19.5 — Debian/RPM packages.
- T19.6 — Windows installer (msi or exe).
- T19.7 — Auto-update mechanism (optional).

### M20: Release (week 43-44)

**Tasks:**
- T20.1 — Security audit pass (sandbox, secrets handling).
- T20.2 — Accessibility audit (UI keyboard navigation, contrast).
- T20.3 — Performance benchmarks documented.
- T20.4 — Final acceptance test: 10 different real-world tasks complete successfully.
- T20.5 — Beta with early users (collect feedback).
- T20.6 — v1.0 release announcement.

---

## How to use this roadmap with Claude

**Recommended workflow:**

1. **Read the spec first.** Have Claude read `README.md` and all RFCs (`rfcs/0001` through `0008`) before implementing anything. This builds shared context.

2. **Pick the next task.** Start at M0.T0.1, work sequentially within milestones. Some tasks within the same milestone can parallelize but most depend on earlier tasks.

3. **Work in small commits.** Each task should be 1-4 hours of work. Commit per task (or sub-task) with descriptive message.

4. **Update CHANGELOG.md** at the end of each milestone with what changed.

5. **Don't skip ahead.** v0.1 → v0.2 ordering matters. Trying to build the editor before the engine works leads to integration pain.

6. **Test before moving on.** Each milestone has acceptance criteria. Don't claim done until they pass.

7. **Use the architecture docs as truth.** When implementing, refer back to specific RFCs and architecture/* files. They're the contract.

## Risk and mitigation

### Risk: ACP integration is harder than expected

ACP is still a young protocol; integrations may have rough edges.

**Mitigation**: Start with a mock ACP agent (M3.T12) that gives deterministic responses. Develop most engine logic against the mock. Only integrate real Claude Code / Codex toward end of M3.

### Risk: GPUI learning curve

GPUI is the framework Zed editor uses, but it's not as documented as egui.

**Mitigation**: M10 has 4 weeks budgeted (not 2). Author already has experience with gpui-component and gpui-navigator from FLUI work. Keep runtime UI scope small in v0.1 (no replay yet).

### Risk: Bootstrapping prompts produce bad results

The 3 bootstrap prompts (Description, Roadmap, Flow Generator) are the make-or-break of the product.

**Mitigation**: Spend disproportionate time iterating on these prompts during M8. Build a fixture-based testing system (M8.T14) that runs prompts against varied inputs and verifies output quality. Don't release v0.1 until 10+ test descriptions produce coherent flows.

### Risk: Sandbox edge cases on Windows

Windows sandboxing (AppContainer + Job Objects) is significantly more limited than Linux/macOS.

**Mitigation**: Windows in v0.1 ships with Tier 1 + Tier 2 only (MCP filtering + path checking). Tier 3 enforcement comes later. Document this clearly. Most users on Windows will probably run vibe-flow in WSL anyway.

### Risk: Author doesn't have time

Solo project, full-time job, two other side projects (Nebula, Surge).

**Mitigation**: Budget realistically (12 months for v1.0, not 6). Don't add features mid-roadmap. Cut scope before extending timeline. Be okay with v0.1 → v0.2 taking longer than v0.1 itself.

## Definitions of done per milestone

| Milestone | Done when |
|-----------|-----------|
| M0 | CI green, all crates build |
| M1 | Core types tested, 80%+ coverage |
| M2 | All storage acceptance criteria pass |
| M3 | Mock + real ACP integration works |
| M4 | Sandbox tests pass on all 3 OSes |
| M5 | End-to-end test (5-node graph) succeeds |
| M6 | All CLI commands work in text + JSON |
| M7 | Full bootstrap via Telegram works |
| M8 | Real run on test crate succeeds |
| M9 | Editor opens + edits flow.toml, round-trips |
| M10 | Runtime UI shows live run progress |
| M11 | Replay scrubber works on long runs |
| M12 | Fork-from-here creates valid new run |
| M13 | Compare two runs side-by-side |
| M14 | AGENTS.md JIT loading saves tokens |
| M15 | Specialized profiles ship and work |
| M16 | Complex branch graphs execute correctly |
| M17 | 10K-event runs perform smoothly |
| M18 | Full user guide published |
| M19 | Binaries available on all major platforms |
| M20 | v1.0 announcement |

---

## Tracking progress

Recommended approach for tracking:

1. Create a `PROGRESS.md` in the repo root.
2. List each milestone with checkboxes for tasks.
3. Update after each work session.
4. Periodically update target dates as reality unfolds.

Example:

```markdown
# Progress

## v0.1 MVP — In progress

### M0: Foundation ✅
- [x] T0.1 — Workspace setup
- [x] T0.2 — CI matrix
- [x] T0.3 — cargo deny
- [x] T0.4 — License files
- [x] T0.5 — Logging
- [x] T0.6 — Error patterns
- [x] T0.7 — Snapshot testing

### M1: Core types — In progress
- [x] T1.1 — Graph types
- [x] T1.2 — Event types
- [ ] T1.3 — Profile types (in progress)
- [ ] ...
```

Don't aim for elaborate project management. Just tick boxes.
