# Project Roadmap

> Local-first meta-orchestrator for AFK AI coding: `describe → approve roadmap/flow → walk away → return to a PR`. Agent-agnostic (ACP), source-agnostic, sandbox-delegated.

> Roadmap lists **what ships** per milestone across the 12-crate workspace. For per-milestone task sequencing and execution detail, run `/aif-plan <milestone>`.

## Milestones

- [x] **Workspace skeleton** — 12-crate Cargo workspace with `surge-core` as I/O-free leaf, dependency graph enforced by cargo
- [x] **Core domain types** — graph / node / edge / profile / sandbox / validation / `RunEvent` / `RunState` / hooks live in `surge-core` with deterministic folds
- [x] **ACP bridge MVP** — dedicated OS thread + single-threaded Tokio + `LocalSet`, `BridgeCommand` / `BridgeEvent` channels, mock agent for tests
- [x] **Per-run SQLite event log** — WAL mode, append-only triggers, materialized views in same transaction, bincode payloads
- [x] **Notification multiplexer** — desktop, webhook, Slack, email, Telegram channels behind a single trait in `surge-notify`
- [x] **Tracker source skeletons** — `TaskSource` trait with GitHub Issues and Linear implementations in `surge-intake`
- [x] **Inbox + intake pipeline** — Triage LLM dispatch, `InboxActionConsumer`, `SnoozeScheduler`, `TicketStateSync`, desktop action listener, Telegram bot skeleton

- [x] **Graph engine GA** — end-to-end execution of every `NodeKind` through `flow.toml` (touches `surge-core`, `surge-orchestrator`, `surge-acp`)
  - `Agent` handler: open ACP session via bridge, inject tools, drive turn loop, validate outcome, close session
  - `HumanGate` handler: render summary, emit `ApprovalRequested`, await `ApprovalDecided`, route by decision
  - `Branch` handler: synchronous predicate evaluation against run state, no LLM
  - `Loop` handler: iterate items collection, enter body subgraph per iteration, accumulate outcomes, support `Backtrack` edges
  - `Subgraph` handler: composition with proper namespace and outcome surfacing
  - `Notify` handler: side-effect dispatch via `surge-notify`, no routing decision
  - `Terminal` handler: append `RunCompleted` / `RunFailed` / `RunAborted`, exit cleanly
  - Injected tool `report_stage_outcome` with **dynamic per-node outcome enum** derived from declared outcomes
  - Injected tool `request_human_input` for mid-stage strategic-decision escalation
  - Hook execution chain: `pre_tool_use`, `post_tool_use`, `on_outcome` (with retry-on-reject), `on_error` — declared in profile, executed by engine
  - Graph invariant validation at load: reachability from `start`, terminal reachable from every non-terminal, no orphan outcomes, edge `kind` valid (Forward / Backtrack / Escalate), profile / template / named-agent references resolve
  - Replay determinism property test (`proptest`): `fold(events[..N])` matches live state byte-for-byte at every `seq`
  - Schema-version migration chain for older event payloads
  - Two execution paths verified: in-process (`engine run` without `--daemon`, single-binary fast path) and daemon-attached (`engine run --daemon --watch`); both share engine code, daemon layer adds IPC + event broadcast
  - 5+ example `flow.toml` archetypes beyond `flow_minimal_agent.toml`: linear-3, multi-milestone-loop, bug-fix-with-Reproduce, refactor-with-Behavior-Characterization, spike (skip Architect / Reviewer)
  - Integration tests against mock ACP agent and at least one real agent (Claude Code or Codex CLI)
  - Criterion bench: stage transition p95 budget, regression-guarded in CI

- [x] **Profile registry & bundled roles** — `~/.surge/profiles/` resolution and shipped role library (touches `surge-core`, `surge-orchestrator`, `surge-cli`)
  - Lookup order: versioned (`implementer-1.0.toml`) → latest (`implementer.toml`) → bundled fallback
  - `extends = "generic@1.0"` shallow-merge inheritance with conflict detection
  - Profile schema: system prompt (Handlebars template), launch config, sandbox intent, allowed tools, declared outcomes, hooks, approval policy
  - Bundled bootstrap roles: Description Author, Roadmap Planner, Flow Generator
  - Bundled execution roles: Spec Author, Architect, Implementer, Test Author, Verifier, Reviewer, PR Composer
  - Specialized variants: Bug-Fix Implementer, Refactor Implementer, Security Reviewer, Migration Implementer
  - Project-level roles: Project Context Author (produces / refreshes `project.md`), Feature Planner (produces roadmap-amendment patches)
  - Asset bundling: bundled profiles embedded in binary via `include_str!` or `rust-embed`
  - `surge profile list` shows registry contents (versioned + bundled) with provenance
  - `surge profile show <name>` renders resolved profile after inheritance
  - `surge profile validate <path>` checks schema + referenced templates / agents
  - `surge profile new <name>` scaffolds a profile from a chosen base
  - Trust / signature story for shared profiles — decide or explicitly defer to post-v0.1
  - Resolution surfaces clear errors when a referenced profile is missing or version mismatch
  - Documentation: profile authoring guide in `docs/`

- [x] **Bootstrap & adaptive flow generation** — three-stage Description → Roadmap → Flow with HumanGate after each (touches `surge-orchestrator`, `surge-core`, `surge-notify`, `surge-cli`)
  - Description Author Agent profile produces `description.md` (goal, context, requirements, out-of-scope)
  - Roadmap Planner Agent profile produces `roadmap.md` (milestones + tasks + dependencies)
  - Flow Generator Agent profile emits validated `flow.toml`, picks profiles from registry
  - HumanGate after each bootstrap stage with edit / redo / approve affordances
  - Archetype detection in Flow Generator: 1-task linear-3, 5–7-task linear-with-Review, multi-milestone outer-loop-wrapping-inner-loops, bug-fix (insert Reproduce), refactor (insert Behavior Characterization), spike (skip Architect / Reviewer)
  - Roadmap-driven runner output: when archetype is multi-milestone, generated `flow.toml` wires outer milestone `Loop` around inner task `Loop`; per-milestone HumanGate cadence configurable in Flow Generator
  - Milestone-progression contract: each milestone exit routes through Verifier + (optional) milestone Reviewer before the next outer-loop iteration
  - Long-lived run support: roadmap runs span hours / days; engine treats event log as the only state across milestones, no in-memory caches survive a stage transition
  - `--template=<name>` skip path: load saved pipeline from registry, skip bootstrap entirely
  - Bootstrap nodes are visible on canvas like any other (not hidden pre-stages); replayable and forkable
  - Generated artifacts stored under `~/.surge/runs/<run_id>/artifacts/` with content addressing
  - Bootstrap output flows into the main pipeline via standard `Edge` mechanics — no special-cased glue
  - Failure mode: if Flow Generator output fails validation, automatic retry with the validation errors fed back to the agent
  - Telemetry: which archetype was chosen, time per stage, edit-loop count
  - Edit-loop cap: after N rejections of same stage, escalate to `request_human_input`
  - Integration tests covering each archetype end-to-end against mock agent

- [x] **Project initialization & stable context** — `surge init` wizard + `surge project describe` + `project.md` as persistent project context (touches `surge-cli`, `surge-orchestrator`, `surge-core`, `surge-persistence`)
  - `surge init` interactive wizard: detects ACP clients on PATH, walks through agent registration, sandbox defaults, worktree path, approvals channels, Telegram setup token
  - `surge init --default` skip path: sensible safe defaults, no questions, fast onboarding
  - ACP client auto-detection: scan PATH for known binaries (`claude`, `codex`, `gemini-cli`) and offer to register found ones with appropriate launch profiles
  - `surge project describe` invokes Project Context Author profile to produce `project.md`
  - `project.md` schema: project name, primary language, framework, git state snapshot, stack detection, key directories, tests location, build commands
  - AGENTS.md / CLAUDE.md / README.md ingestion summarized into project context for downstream Description Author / Implementer prompts
  - Content-addressed `project.md` refresh: re-running `surge project describe` updates only on real change, no spurious diffs
  - Project-level memory: every run reads latest `project.md` automatically as part of agent context binding
  - `surge.toml` schema covers wizard outputs (agents, sandbox, worktrees, approvals, Telegram, MCP servers) with sensible defaults
  - Annotated `surge.example.toml` aligned with wizard output (local / npx / custom / TCP / MCP-flavored agent entries)
  - Wizard idempotency: re-running on existing project shows current state and lets user edit individual sections
  - Smoke test: `surge init --default` + `surge project describe` + `surge engine run examples/flow_minimal_agent.toml` works end-to-end on a fresh repo
  - First-run UX polish: clear error messages on missing dependencies (no agent on PATH, not a git repo, etc.) with actionable next steps
  - Documentation: `getting-started.md` updated to use wizard flow as canonical onboarding path

- [x] **Artifact format & convention library** — canonical output contract for every role; surge owns *what* artifacts look like, agents own *how* they think (touches `surge-core`, `surge-orchestrator`, profiles registry, `docs/`)
  - Bundled `flow.toml` templates extracted from `surge-spec/templates.rs`: Feature / Bugfix / Refactor / Performance / Security / Docs / Migration, registered for `--template=<name>` consumption
  - Profile output schemas: Description Author → markdown sections (Goal / Context / Requirements / Out-of-Scope); Roadmap Planner → `surge_core::Roadmap` typed output; Spec Author → `surge_core::Spec` typed output with subtasks + acceptance criteria; Architect → ADR with frontmatter
  - Output validation as `on_outcome` reject hook (uses Graph engine GA primitive): post-stage validator runs against schema, retries on schema violation
  - Planner prompt templates from `surge-orchestrator/planner.rs` ported into profile system prompts (Description Author, Roadmap Planner, Spec Author, Architect, Implementer, Verifier, Reviewer, PR Composer)
  - Conventions doc (`docs/conventions/`): description, requirements, ADR, story file, plan, spec, roadmap formats with worked examples
  - Story-file convention preserved: `stories/story-NNN.md` referenced from `Subtask.story_file`, format documented
  - ADR convention: `docs/adr/<NNNN>-<slug>.md` with TOML frontmatter (status, deciders, date, supersedes)
  - Acceptance-criteria pattern preserved: machine-readable list inside each subtask, consumed by Verifier profile
  - Cross-agent uniformity test: same flow run on Claude Code vs Codex CLI vs mock agent produces format-equivalent artifacts (golden-file comparison in CI)
  - Schema versioning: artifacts carry `schema_version`; older ones run through migration chain on read
  - Explicit boundary doc: agents may use their internal plan modes (Claude Code `EnterPlanMode`, superpowers `writing-plans`, etc.) — surge enforces *output contract*, not *internal process*
  - Artifact authoring guide for new role types in `docs/`
  - Validator failure UX: schema-rejected outputs surface a concise diff in Telegram cards so the user sees *what failed*

- [x] **Roadmap amendments via `surge feature`** — live mutation of an active roadmap or follow-up runs from a completed one (touches `surge-cli`, `surge-orchestrator`, `surge-core`, `surge-persistence`, `surge-notify`)
  - `surge feature describe <prompt>` runs the Feature Planner profile against a project or run roadmap target and stores a normalized `roadmap-patch.toml`
  - Feature Planner produces a typed `RoadmapPatch`: insertion point, new milestone/task items, dependencies, rationale, conflicts, and lifecycle status
  - `RoadmapPatch` is a typed structure in `surge-core` with `schema_version`, validation, content hashing, and pure apply semantics
  - Patch approval loop records approve / edit / reject decisions with optional conflict choices and an edit-loop cap
  - Approved typed project-roadmap patches apply to the selected roadmap artifact/file and preserve replay through content-addressed artifacts and events
  - Approved active-flow patches can produce graph revisions with new milestone/task nodes, rewired edges, validation, and rollback-on-failure semantics
  - `RoadmapPatchDrafted`, `RoadmapPatchApprovalRequested`, `RoadmapPatchApprovalDecided`, `RoadmapPatchApplied`, `RoadmapUpdated`, and `GraphRevisionAccepted` events live in `surge-core`
  - Target resolver detects project, active-run, deferred, and terminal-run amendment points from registry/run status and artifact views
  - Active runners observe accepted graph revisions at safe boundaries; terminal or follow-up-only targets create follow-up run requests from appended work
  - Idempotency: registry-level content-hash dedup keeps duplicate `RoadmapPatch` rows stable
  - Conflict detection: running/completed roadmap conflicts surface stable codes and operator choices: defer to next milestone, abort current run, create follow-up run, or reject patch
  - Notification payloads cover patch approval, apply, runner pickup, follow-up run creation, conflicts, and rejection; Telegram can render richer cards from the same schema later
  - CLI mirrors: `surge feature list`, `surge feature show <id>`, `surge feature reject <id>`, plus `--conflict-choice` on describe/reject
  - Regression tests cover malformed patch rejection, duplicate idempotency, running conflicts, follow-up request creation, CLI mirrors, replay/snapshot views, and notification rendering
  - Documentation: amendment lifecycle and command reference live in `docs/workflow.md`, `docs/cli.md`, and `docs/conventions/roadmap.md`

- [x] **Legacy pipeline retirement** — remove `surge-spec` crate and the parallel-execution path in `surge-orchestrator` (touches `surge-spec`, `surge-orchestrator`, `surge-cli`, workspace root)
  - **Parity checklist** completed before any deletion: every behavior of the legacy pipeline has a verified equivalent in the graph executor
  - `surge-spec/graph.rs` `DependencyGraph::topological_batches` → graph executor `Loop` over a `Subtask` collection with declared dependencies
  - `surge-orchestrator/qa.rs` checks → `Verifier` profile + `on_outcome` validation hook
  - `surge-orchestrator/retry.rs` and `circuit_breaker.rs` → engine-level retry policy bound to `on_error` hook
  - `surge-orchestrator/parallel.rs` and `schedule.rs` → graph executor's batched `Loop` execution
  - `surge-orchestrator/{pipeline,phases,planner,gates,executor,conflict,budget,project,context}.rs` mapped to engine equivalents and replacements verified
  - `surge-spec/{builder,parser,validation}.rs` legacy I/O retired (templates already moved by Artifact format milestone)
  - Dual-write/read deprecation window with telemetry on which path is exercised
  - Deprecation warning on every `surge-spec` use during the window
  - `surge spec` subcommands removed from `surge-cli` after deprecation window closes
  - `surge migrate-spec <path>` CLI auto-translates old specs to `flow.toml` where deterministic; surfaces ambiguous cases for human edit
  - External `flow.toml` migration guide in `docs/` for users who authored specs manually
  - All tests using `surge-spec` ported to graph-executor equivalents or removed
  - `surge-spec` crate deleted from workspace; root `Cargo.toml` `members =` updated
  - **Cleanup pass**: drop dead root-level files in `surge-orchestrator` left over from the legacy path
  - **Restructure**: collapse remaining cross-cutting code into `engine/` or extracted helpers
  - Documentation cleanup: remove `surge-spec` mentions from `docs/ARCHITECTURE.md`, `DESCRIPTION.md`, `CLAUDE.md`, `.ai-factory/ARCHITECTURE.md`
  - Final acceptance: `cargo tree | grep surge-spec` empty; `cargo build --workspace` clean; zero deprecation warnings

- [x] **Sandbox delegation matrix** — `SandboxIntent` mapped to every supported runtime (touches `surge-acp`, `surge-core`, `surge-orchestrator`, `surge-cli`)
  - Map `read-only` to native flags on Claude Code, Codex CLI, Gemini CLI
  - Map `workspace-write` to native flags on all 3 runtimes
  - Map `workspace+network` to native flags on all 3 runtimes
  - Map `full-access` to native flags on all 3 runtimes (with strong warning surface)
  - `custom` intent: TOML-defined launch flags pass-through with validation
  - `surge doctor` reports unsupported (intent × runtime) combos with actionable hint
  - No silent downgrades — unsupported combos refuse to run, not weakened
  - Per-runtime version pinning: surge declares minimum supported version of each runtime; `doctor` warns on older
  - Elevation roundtrip: `SandboxElevationRequested` event → notify channel → `ApprovalDecided` → resume session via ACP permission callback
  - Negative tests: blocked elevation surfaces as `StageFailed` with reason
  - Negative tests: unsupported combo surfaces in `doctor` and refuses run start
  - `surge doctor agent <name>` runs a smoke session against the runtime
  - Documentation: sandbox capability matrix table in `docs/`
  - Audit logging: every elevation request + decision recorded with command summary

- [x] **MCP server lifecycle** — production-grade `surge-mcp` (touches `surge-mcp`, `surge-orchestrator`, `surge-cli`, `surge-daemon`) — landed via plan `docs/plans/2026-05-17-001-feat-mcp-server-lifecycle-plan.md`, see [ADR 0014](../docs/adr/0014-mcp-server-lifecycle.md). Structured `ServiceError` crash detection, redacted bounded stderr capture, exponential-backoff restart policy with capped attempts + `EscalationRequested` give-up (cockpit-visible), periodic health monitor, deterministic per-run teardown, per-server replay attribution (schema v3), reserved injected-tool names, sandbox-intent plumbing, and `surge mcp list/start/stop/logs`. Deferred with rationale (ADR-0014): `surge mcp stop` is an explicit idempotent ack under per-run isolation (live-run halt is `surge run abort`); OS-level sandboxing of MCP children is delegated to the runtime per ADR-0006 (`FullAccess`/`WorkspaceNetwork` MCP binaries run unconstrained — operator-trusted only); persistent shared servers (`McpServerRef::isolation = Shared`) and run-scoped `surge mcp logs` deferred to a future shared-server milestone; supervisor extraction across inbox/cockpit/MCP deferred.
  - `rmcp` stdio child-process transport wired end-to-end
  - Registry-driven launch: `mcp.toml` lists servers with command, args, env, sandbox intent
  - Daemon starts MCP children with run lifecycle; stops on terminal outcome
  - Per-server isolation: one child per `mcp.toml` entry, no shared state
  - Tool delegation surfaced as `ToolCall` / `ToolResult` events for replay parity
  - Health checks: periodic `tools/list` ping; mark unhealthy after N failures
  - Restart policy on child crash: exponential backoff, capped attempts, then escalate
  - Structured logs from MCP child stderr captured via `tracing`
  - `surge mcp list` shows configured servers and runtime status
  - `surge mcp start / stop <name>` manual lifecycle control
  - `surge mcp logs <name>` tails captured stderr
  - Tool-name conflict resolution: surge-injected tools (`report_stage_outcome`, `request_human_input`) win on collision
  - Integration test against a known MCP server (filesystem MCP)
  - Sandbox: MCP children honor the run's `SandboxIntent` where the runtime supports it

- [x] **Telegram cockpit production-ready** — first-class approval surface (touches `surge-notify`, `surge-daemon`, `surge-cli`, `surge-persistence`) — landed via plan `.ai-factory/plans/telegram-cockpit-production-ready.md`. New `surge-telegram` crate hosts the bot loop, callback router, card store, recovery reconciler, rate limiter, and `cockpit::production` adapters; daemon spawns it next to `TgInboxBot`. Sandbox elevation card and webhook receiver explicitly deferred (Out of Scope §). The production teloxide-polling `Stream` adapter is wired into `CockpitWiring` but the live polling source is a one-line `polling_default(bot)` swap pending a follow-up (engine-tap dispatch and snooze re-emit are live).
  - `surge telegram setup` with ephemeral binding token persisted in registry SQLite
  - Per-user authorization — only paired chat IDs receive cards; admission failures logged
  - Bootstrap card: Description / Roadmap / Flow stage previews with approve / edit / redo buttons
  - HumanGate card: stage summary + approve / reject buttons
  - Sandbox elevation card: command preview + risk notes + approve / deny
  - Progress card: live stage / token consumed / outcome (auto-refresh on event log change)
  - Completion card: PR link, summary, cost, duration
  - Failure card: stage that failed, error excerpt, retry / abort buttons
  - Long-poll default via `teloxide`; webhook receiver via `tiny_http` as opt-in
  - Card update via `editMessageText` rather than spam-posting new messages
  - Inline-keyboard buttons time out gracefully (e.g., approve clicked after 24h handled cleanly)
  - Telegram-side `/run`, `/status`, `/abort`, `/runs` commands route to event-log queries
  - Rate-limit handling against Telegram Bot API
  - Snooze ergonomics from inbox subsystem extended to cockpit cards (`/snooze 1h` etc.)

- [x] **Tracker automation tiers** — L0–L3 honored on GitHub Issues and Linear (touches `surge-intake`, `surge-orchestrator`, `surge-daemon`) — landed via plan `.ai-factory/plans/tracker-automation-tiers.md`, see [ADR 0013](../docs/adr/0013-tracker-automation-tiers.md). L3 PR-readiness check and `CadenceController` source-loop wiring deferred to follow-up tasks with explicit blocked/no-op fallbacks today.
  - L0 (`surge:disabled` or label absent): tracker ignores ticket entirely
  - L1 (`surge:enabled`, default): full bootstrap; user approves before run starts
  - L2 (`surge:template/<name>`): skip bootstrap, use named template directly
  - L3 (`surge:auto`): full automation including merge on success
  - `surge-priority/<level>` label honored: high / medium / low affects polling cadence
  - Surge writes only labels and comments — never ticket status (open / closed / in-progress)
  - Comment template: triage decision (enqueued / duplicate / out-of-scope / unclear)
  - Comment template: run start with run id and link to status surface
  - Comment template: completion with PR link, summary, token cost
  - Comment template: failure with stage and reason
  - Polling cadence: L1 5min, L2 2min, L3 1min; exponential backoff on rate limits
  - Idempotency: `(tracker, ticket_id, event_kind, run_id)` dedup key prevents duplicate comments on retry
  - Per-tracker quirks: GitHub via `octocrab` (rate limit), Linear via `lineark-sdk` (cursor pagination)
  - `surge intake list` shows pending / running / completed tickets across trackers
  - Ticket-as-master integrity: external state changes on ticket (close, reassign) reflected in inbox state
  - L3 merge-on-success guarded by all-checks-green and review-approved policy

- [x] **Crash recovery** — daemon survives restarts with no AFK regression, **v0.1 blocker** (touches `surge-daemon`, `surge-persistence`, `surge-orchestrator`, `surge-notify`) — landed via `surge-daemon::recovery`: a pure `decide_action` policy (skip-active / skip-terminal / reconcile-terminal / mark-failed-worktree-lost / flag-stuck / resume), a read-only `plan_recovery` registry scan, and an `execute_recovery` executor wired into daemon startup through the shared admission + broadcast registry (so recovered runs publish `RunFinished` globally). `surge daemon recover [--dry-run]` exposes the inspector; `Storage::set_run_status` added for reconciliation. Full model + decision table in [`docs/crash-recovery.md`](../docs/crash-recovery.md). Deferred: a dedicated `kill -9`/power-cut fault-injection harness (WAL durability is configured and the resume-from-log path is integration-tested; the explicit process-kill checkpoint harness is a follow-up).
  - Daemon startup scans `runs` table for non-terminal status (`run_status NOT IN (Completed, Failed, Aborted)`)
  - For each non-terminal run, fold event log to current state via `surge-core::fold`
  - Per-stage recovery decisions: `Agent` mid-turn → retry; `HumanGate` pending → re-emit approval; `Notify` mid-flight → retry; `Terminal` not yet appended → append on stage completion
  - Approval re-emission deduplicates against still-open Telegram cards via card-id correlation
  - Approval re-emission deduplicates desktop / Slack / email channels likewise
  - WAL checkpoint behavior verified under kill -9 / power-cut fault injection (test harness)
  - `surge daemon recover --dry-run` lists recovery decisions without side effects
  - Recovery telemetry: how many runs recovered, how many failed to recover, per-stage histogram
  - Stuck-run detection: a run with no events for >24h gets human-attention card
  - Worktree consistency check: if run worktree was lost, mark run as failed with clear error
  - Schema-version handling: any old event payloads run through migration chain before fold
  - PID file + Unix socket / Windows named-pipe stale-handle cleanup on startup
  - Recovery idempotency: re-running recovery on already-recovered runs is a no-op

- [ ] **v0.1 public release** — first announceable cut (touches workspace root, `surge-cli`, `docs/`, CI) — **in progress.** Landed: `surge --version` with git sha + commit date (build.rs), panic crash-report hook, `surge.toml`/`flow.toml`/event-payload schemas frozen at v1 with a documented bump plan ([`docs/schema-versioning.md`](../docs/schema-versioning.md)), `cargo deny` license gate ([`deny.toml`](../deny.toml) + [`THIRD_PARTY.md`](../THIRD_PARTY.md), wired into `security.yml`), MSRV (1.96) CI job, and the v0.1 release-notes draft incl. zero-by-default telemetry posture ([`docs/release-notes-v0.1.md`](../docs/release-notes-v0.1.md)). The 3-OS smoke matrix already exists in `ci.yml`. Remaining (external infra, not codeable in-tree): `cargo publish` of publishable crates, Homebrew tap + Scoop manifest, live multi-OS CI execution, and the recorded end-to-end run against a real public repo.
  - `surge.toml` schema frozen with `schema_version = 1`; documented in `docs/`
  - `flow.toml` schema frozen with `schema_version = 1`; documented in `docs/`
  - Schema migration plan documented for future bumps
  - Smoke-test matrix in CI: Ubuntu, macOS, Windows × stable Rust × MSRV
  - Install docs: `cargo install surge-cli`, homebrew tap, scoop manifest for Windows
  - Dual-license headers (MIT / Apache-2.0) verified across all crates via `cargo deny` rule
  - License compliance: third-party licenses inventory in `THIRD_PARTY.md`
  - Release notes drafted for v0.1
  - Announcement post drafted (target audience: AFK AI coding folks)
  - `cargo publish` for publishable crates; everything else stays workspace-internal
  - `surge --version` includes git sha and build date
  - Crash-report path: panic handler captures backtrace, suggests filing issue with redacted log
  - Telemetry posture: zero by default; document explicit opt-in if added later
  - First-run UX: `surge init` walks user through config, agent install, telegram setup
  - End-to-end smoke against a real public repo recorded as an example run

- [ ] **Replay & fork-from-here UI** — post-v0.1 polish over the same fold primitive (touches `surge-ui`, `surge-persistence`, `surge-cli`) — **CLI mirror started.** `surge engine replay <run_id> --seq <N>` folds the event log to seq `N` and prints the run state (reusing the tested `aggregate_status` primitive). Building it surfaced and fixed a real reader bug: `read_events` bound `EventSeq(u64::MAX)` as `-1` in SQLite (open-ended reads returned zero rows), which also silently degraded `current_status` (cockpit `/status`) and crash-recovery stuck/reconcile detection. The fork half now landed too (v0.3 M1): `surge engine fork <run_id> --seq N` + the `engine::fork::fork` core copy the event prefix, inherit the parent snapshot so the child resumes at the fork point, and record `ForkCreated` lineage. Remaining: pre-fork prompt/profile edits, the GPUI scrubber/visual states, and live-vs-replay mode toggle (see the **v0.3 — Time-travel** section).
  - Scrubber timeline rendered from event log with `seq` slider
  - Live mode disables scrubber; Replay mode enables it; clear visual differentiation
  - Fork CTA copies events `1..N` into a new run id; new git worktree at the same commit
  - Pre-fork edit affordance: prompt override per Agent node, profile change per node
  - Diff viewer integration into replay panel (tied to `ToolCall` / `ToolResult` events)
  - Artifact viewer integration: render `description.md`, `roadmap.md`, `flow.toml` at any seq
  - Visual states: completed nodes (teal), active node (pulsing), future nodes (dimmed)
  - Edge highlighting on traversed edges per fold step
  - Keyboard shortcuts: arrow keys for prev/next event, `f` for fork, `space` for play/pause
  - Performance: scrubber update under 16ms (60fps) for runs up to 10k events
  - GPUI integration extends existing `surge-ui::screens::live_execution`
  - CLI mirror: `surge engine replay <run_id> --seq <N>` prints state at that seq
  - CLI mirror: `surge engine fork <run_id> --seq <N>` creates the forked run from terminal

## v0.2 — Autonomy: AFK → PR for real

> North star: make the core thesis — *describe → approve → walk away → return to a PR* — trustworthy for real, unattended work. v0.1 proved the loop drives a live ACP agent end-to-end; v0.2 hardens the autonomous path so it can run for hours without a human babysitting cost, merge, or agent quirks. Per-milestone task sequencing: run `/aif-plan <milestone>`.

- [ ] **Live budget enforcement** — stop burning money/tokens unattended (touches `surge-core`, `surge-orchestrator`, `surge-persistence`, `surge-notify`)
  - Per-run and per-milestone budget overrides (USD + tokens) layered over the existing global `AnalyticsConfig.budget_*`; resolution order global → run → milestone, surfaced in `surge.toml` / `flow.toml`
  - Engine accumulates ACP session usage (`unstable_session_usage`) into run state at every stage boundary; cumulative cost/tokens are a deterministic fold, no wall-clock
  - Budget evaluation at stage transitions: warn at `budget_warn_threshold` → `Notify`; exceed → policy-driven action (escalate via `request_human_input`, pause-for-approval, or abort) chosen by config
  - New `surge-core` events: `BudgetWarningRaised`, `BudgetExceeded`, `BudgetDecision` (resume-with-raise / abort), with `schema_version` + migration entry
  - Budget state visible in `surge analytics`, the Telegram budget card (warn/exceeded with raise/abort buttons), and `surge engine replay`
  - Idempotency: re-evaluating budget on an already-warned/exceeded run is a no-op until the threshold is crossed again
  - Tests: fold determinism for usage accumulation, warn/exceed transition table, escalate-vs-abort policy, replay parity, threshold-crossing idempotency
  - Documentation: budget model + policy matrix in `docs/`

- [ ] **L3 auto-merge completion** — close "return to a *merged* PR" (touches `surge-daemon`, `surge-intake`, `surge-git`, `surge-orchestrator`)
  - Real PR-readiness gate (replaces the no-op fallback): all-required-checks-green + review-approved, polled via `octocrab` (GitHub) with stable status codes
  - `CadenceController` source-loop wiring (replaces the deferred stub): L1 5min / L2 2min / L3 1min polling with exponential backoff on rate limits
  - Merge-on-success guarded by the readiness gate; merge method configurable (squash default, matching repo convention)
  - Failure modes: checks red / review missing / merge conflict → tracker comment + `EscalationRequested`, never a silent stall
  - Idempotency: `(tracker, ticket_id, merge, run_id)` dedup prevents double-merge on retry; merge already-done is a clean no-op
  - Audit: every readiness decision + merge attempt recorded as events with the command/PR summary
  - Tests: gate-green-merges, gate-red-blocks-and-escalates, conflict-escalates, idempotent double-merge, cadence backoff
  - Documentation: L3 lifecycle in `docs/tracker-automation.md` (replace the "out of scope (next milestone)" note)

- [ ] **Multi-agent breadth validated live** — agent-agnostic in fact, not just in registry (touches `surge-acp`, `surge-orchestrator`, `surge-cli`) — **codeable core landed** (#76 per-runtime launch-arg unit tests + [`docs/agent-runtimes.md`](../docs/agent-runtimes.md) support matrix; `surge doctor agent <name>` real smoke with spawn/handshake/auth/prompt stage classification, env-gated `SURGE_DOCTOR_REAL`; per-runtime auth diagnostics reuse `AgentAuthenticationFailed`). **Operator-gated remainder:** live Codex/Gemini launch validation + cross-agent golden compare (need the runtimes installed + logged in).
  - Live launch validation for Codex and Gemini ACP adapters through handshake → `new_session` → `session/prompt` (the bar v0.1 hit for Claude), env-gated like `real_acp_smoke`
  - Fix any launch/arg/headless-mode gaps surfaced per runtime (mirror of the Claude headless-settings + npx-resolve work)
  - `surge doctor agent <name>` runs a real smoke session per registered runtime and reports the failure stage precisely (spawn / handshake / auth / prompt)
  - Cross-agent artifact uniformity: same flow on Claude vs Codex vs Gemini vs mock produces format-equivalent artifacts (golden-file compare in CI where an agent is available)
  - Per-runtime auth-failure diagnostics reuse the `AgentAuthenticationFailed` classification
  - Tests: per-adapter launch-arg unit tests, doctor smoke-stage classification, golden artifact compare
  - Documentation: per-runtime support matrix + known quirks in `docs/`

- [ ] **Durability proof — fault-injection harness** — recovery survives the worst case (touches `surge-daemon`, `surge-persistence`, test infra) — **slice 1 landed** (#77): debug-only `SURGE_CHECKPOINT_EXIT` seam aborts the process uncleanly right after `StageEntered` commits; a real `surge engine run` subprocess is killed mid-run and `surge engine replay` proves the WAL log survives and folds to the partial mid-run state. **Remaining (follow-up):** the full daemon kill → restart → recover cycle across the checkpoint matrix, and the true power-cut case (`synchronous = FULL` vs the current `NORMAL`) — see [`docs/crash-recovery.md`](../docs/crash-recovery.md).
  - Harness that kills the daemon process (SIGKILL / simulated power-cut) at defined checkpoints mid-run, restarts, and asserts recovery resumes to the correct folded state
  - WAL checkpoint behavior verified: no event-log corruption, no lost committed events, no duplicate appends after restart
  - Checkpoint matrix: mid-Agent-turn, pending-HumanGate, mid-Notify, pre-Terminal-append — each asserts the v0.1 recovery decision policy
  - Recovery idempotency under repeated kills (kill during recovery itself)
  - CI: harness runs on Linux (signals) with a Windows-named-pipe stale-handle variant
  - Closes the v0.1 deferred "`kill -9` / power-cut fault-injection harness"

- [ ] **Recorded real-repo end-to-end** — the proof artifact (touches `docs/`, `examples/`, CI-adjacent) — **CI guard + operator script landed**: the mock-agent `init → describe → run` smoke runs in CI (`onboarding_smoke` in `examples_smoke.rs`) so the script can't rot; [`docs/recorded-e2e.md`](../docs/recorded-e2e.md) is the reproducible operator procedure for the real-repo + live-agent + PR run. **Operator-gated remainder:** the actual recorded run against a real public repo (needs an authenticated runtime — not runnable in CI).
  - Scripted end-to-end against a real public repo: `init → describe → approve → run → PR` with a working agent runtime, producing a real PR
  - Recorded as a reproducible example (asciinema/log + the resulting flow + artifacts) under `examples/` / `docs/`
  - Documents the one external prerequisite (agent runtime auth) and the exact commands
  - Doubles as the v0.1 release-notes "end-to-end smoke against a real public repo recorded as an example run" deliverable
  - Smoke variant wired into CI with the mock agent so the script itself can't rot

## v0.3 — Time-travel: replay, fork-from-here, run history

> North star: the §1 differentiator made real — *append-only event log → replay, time-travel, fork-from-here*. A run stops being a black box and becomes a navigable, forkable, queryable artifact, so a run that derails at stage 6/8 is forked from just-before-6 with a fix instead of re-run from scratch. Engine + CLI core is deterministic and CI-verifiable; the GPUI surface sits on top of it. Per-milestone task sequencing: run `/aif-plan <milestone>`.

- [ ] **M1 — Fork-from-here (engine + CLI)** — **core + pre-fork edits landed.** `engine::fork::fork` copies parent events `1..=N` into a fresh run, inherits the parent's latest snapshot (child resumes at the fork point, not `graph.start`), and records `ForkCreated` lineage on the parent; `surge engine fork <run> --seq N` exposes it (prints new id + copied count + inspect/resume hints). Pre-fork edits `--prompt <node>=<text>` (append to an Agent node's system prompt) and `--profile <node>=<key>` (swap profile) rewrite the child's materialized graph + hash, validated all-or-nothing (unknown/non-Agent target → `ForkInvalid`; runs with mid-run graph revisions rejected for now). Tested at the orchestrator level (prefix copy + fold-equality, snapshot-inherit resume position, bounds + RunStarted-first rejection, prompt/profile edit + reject cases) and end-to-end through the binary.
  - **Remaining:** optional new-worktree-at-fork-commit + auto-resume convenience
  - **Remaining:** fork lineage surfaced in run views (parent ↔ children)
  - **Remaining:** pre-fork edits for runs with mid-run graph revisions
- [ ] **M2 — Replay-at-seq enrichment** — extend `surge engine replay` with node status (completed/active/future), traversed edges, and cost-so-far; artifact-at-seq (`--artifact <name>` renders `description.md` / `roadmap.md` / `flow.toml` as of seq N); diff-at-seq; `--format json` for UI/script consumption
- [ ] **M3 — Run history & cross-run analytics** — `surge runs` list/show with a lineage tree (parent ↔ forks); query by archetype / profile / agent / outcome; failure-pattern aggregation and cost / duration / outcome histograms over the run registry
- [ ] **M4 — GPUI cockpit: scrubber + fork CTA** — wire replay-at-seq + fork into `surge-ui::screens::live_execution`: seq slider, live-vs-replay mode, completed/active/future node coloring, traversed-edge highlight, fork CTA, diff + artifact viewers. Surface-level, operator-verified (GUI is not CI-runnable). Subsumes the v0.1-era "Replay & fork-from-here UI" milestone

## Completed

| Milestone | Date |
|-----------|------|
| Workspace skeleton | 2026-05-06 |
| Core domain types | 2026-05-06 |
| ACP bridge MVP | 2026-05-06 |
| Per-run SQLite event log | 2026-05-06 |
| Notification multiplexer | 2026-05-06 |
| Tracker source skeletons | 2026-05-06 |
| Inbox + intake pipeline | 2026-05-06 |
| Graph engine GA | 2026-05-07 |
| Profile registry & bundled roles | 2026-05-07 |
| Bootstrap & adaptive flow generation | 2026-05-09 |
| Project initialization & stable context | 2026-05-10 |
| Artifact format & convention library | 2026-05-11 |
| Roadmap amendments via `surge feature` | 2026-05-11 |
| Sandbox delegation matrix | 2026-05-13 |
| Legacy pipeline retirement | 2026-05-13 |
| MCP server lifecycle | 2026-05-17 |
| Telegram cockpit production-ready | 2026-05-30 |
| Tracker automation tiers | 2026-05-30 |
| Crash recovery | 2026-05-30 |
