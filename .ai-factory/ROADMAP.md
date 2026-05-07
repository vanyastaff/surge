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

- [ ] **Bootstrap & adaptive flow generation** — three-stage Description → Roadmap → Flow with HumanGate after each (touches `surge-orchestrator`, `surge-core`, `surge-notify`, `surge-cli`)
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

- [ ] **Project initialization & stable context** — `surge init` wizard + `surge project describe` + `project.md` as persistent project context (touches `surge-cli`, `surge-orchestrator`, `surge-core`, `surge-persistence`)
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

- [ ] **Artifact format & convention library** — canonical output contract for every role; surge owns *what* artifacts look like, agents own *how* they think (touches `surge-core`, `surge-orchestrator`, profiles registry, `docs/`)
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

- [ ] **Roadmap amendments via `surge feature`** — live mutation of an active roadmap or follow-up runs from a completed one (touches `surge-cli`, `surge-orchestrator`, `surge-core`, `surge-persistence`, `surge-notify`)
  - `surge feature describe <prompt>` opens a chat session with Feature Planner profile to clarify scope before patch generation
  - Feature Planner produces a `RoadmapPatch`: insertion point (which milestone, which position), new milestone / task items, dependencies, rationale
  - `RoadmapPatch` is a typed structure in `surge-core` with `schema_version`
  - HumanGate on patch before apply: approve / edit / reject with redo loop
  - Approved patch applied to `roadmap.md`: versioned suffix (`vNext`) or commit-based history; original preserved for replay
  - Approved patch applied to active `flow.toml`: new milestone / task nodes inserted, edges rewired, validation re-run, rollback on validation failure
  - New `RoadmapUpdated` event in `surge-core` event types (joins `RunStarted`, `StageEntered`, etc.)
  - Active runner detection: daemon checks if a run is currently executing this roadmap via run-status query
  - If active and roadmap-flow execution enabled: emit `RoadmapUpdated`; runner picks up new pending work in next outer milestone Loop iteration
  - If terminal or roadmap-flow disabled: spawn follow-up run from the appended portion only — completed history is never mutated
  - Idempotency: re-applying the same `RoadmapPatch` is a no-op (content-hash dedup)
  - Conflict detection: patch referencing a milestone that's already running surfaces a clear conflict to user with options (defer to next milestone, abort current, follow-up run)
  - Telegram surface: notification card on patch approval, runner pickup, or follow-up run creation
  - CLI mirrors: `surge feature list` (pending patches), `surge feature show <id>`, `surge feature reject <id>`
  - Integration tests: amendment during running roadmap, amendment after terminal roadmap, malformed patch rejection, conflicting patch on running milestone
  - Documentation: amendment lifecycle diagram in `docs/workflow.md`-aligned format

- [ ] **Legacy pipeline retirement** — remove `surge-spec` crate and the parallel-execution path in `surge-orchestrator` (touches `surge-spec`, `surge-orchestrator`, `surge-cli`, workspace root)
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

- [ ] **Sandbox delegation matrix** — `SandboxIntent` mapped to every supported runtime (touches `surge-acp`, `surge-core`, `surge-orchestrator`, `surge-cli`)
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

- [ ] **MCP server lifecycle** — production-grade `surge-mcp` (current crate is skeleton — touches `surge-mcp`, `surge-orchestrator`, `surge-cli`)
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

- [ ] **Telegram cockpit production-ready** — first-class approval surface (touches `surge-notify`, `surge-daemon`, `surge-cli`, `surge-persistence`)
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

- [ ] **Tracker automation tiers** — L0–L3 honored on GitHub Issues and Linear (touches `surge-intake`, `surge-orchestrator`, `surge-daemon`)
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

- [ ] **Crash recovery** — daemon survives restarts with no AFK regression, **v0.1 blocker** (touches `surge-daemon`, `surge-persistence`, `surge-orchestrator`, `surge-notify`)
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

- [ ] **v0.1 public release** — first announceable cut (touches workspace root, `surge-cli`, `docs/`, CI)
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

- [ ] **Replay & fork-from-here UI** — post-v0.1 polish over the same fold primitive (touches `surge-ui`, `surge-persistence`, `surge-cli`)
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
  - CLI mirror: `surge replay <run_id> --seq <N>` prints state at that seq
  - CLI mirror: `surge fork <run_id> --seq <N>` creates the forked run from terminal

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
