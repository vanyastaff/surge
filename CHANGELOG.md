# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — Provider rate-limit parking: park, wake, and see it in the inbox

- **A run no longer dispatches into a provider it already knows is
  rate-limited.** Before starting a node, Surge checks whether the node's
  agent runtime has a known-exhausted rate-limit window; if so, the run
  parks (`RunStatus::Parked`) with a recorded wake time instead of
  dispatching and finding out from a fresh 429. A rate limit hit *during*
  a dispatch parks the run immediately too, ahead of any configured
  on-error retry, so a rate-limited node no longer burns a retry attempt
  on every pass through a retry loop before the run finally gives up.
- **Parked runs wake themselves up.** A scheduler in `surge daemon`
  resumes a parked run once its recorded wake time passes — no operator
  action and no daemon restart required. The resumed run's budget re-arms
  exactly the way a manually-resumed run's already does. When the
  provider gave no usable reset time, Surge parks on a configurable blind
  backoff instead (see the new `[capacity]` section of `surge.toml`
  below), and raises a visible escalation (a desktop notification and an
  inbox-visible flag) after several blind parks in a row with no
  successful dispatch between them — a signal that something may be stuck
  outright, not merely rate-limited.
- **`surge inbox` shows parked runs, and why they're waiting.** A new
  WAITING group lists every parked run alongside its wake time and
  whether that time is an actual provider-observed reset or a policy
  guess. `--format json` carries the same two facts (`wake_at`,
  `wake_basis`) as structured fields on the entry, not only as the
  `"waiting"` status string.
- **New `[capacity]` section in `surge.toml`**: `blind_backoff` (how long
  to park on a guess when no provider reset time is known — delete the
  line to opt out of blind parking entirely), `blind_park_limit` (how
  many consecutive blind parks are tolerated before Surge escalates), and
  `jitter_max` (spreads multiple runs parked on the same exhausted
  runtime across a short window instead of all waking in the same
  instant). [ADR 0016](docs/adr/0016-capacity-parking-and-wake.md).

Deliberately not shipped, named so an operator does not assume otherwise:
**no rotation across accounts** — Surge does not track more than one login
per agent runtime today, so an exhausted runtime parks; it does not fail
over to a different account of the same agent, and there is no
`surge.toml` field to enable one. **A node's estimated work does not
factor into the parking decision** — the comparison machinery exists
internally, but nothing in this delivery feeds it real per-node cost
data, so today's decision is only "is the runtime currently exhausted,"
never "would this specific node's work fit in what's left of the
window." **The capacity window is learned only from an observed
rate-limit error, never from an agent's own usage reporting** — no
current agent-runtime integration exposes a rate-limit window or reset
time for Surge to read proactively, so a runtime Surge has never seen
fail reads as clean, and Surge cannot warn before the first 429 happens.

### Added — Skill binding, trust gate, and the `SkillBound` event

- **`AgentConfig::declared_skills()`** — a node's `skills = [...]` declaration
  (read from `custom_fields["skills"]`, an array of `surge_core::skill::SkillRef`)
  resolves and binds at stage entry, exactly like a context `Binding`
  (`project.md`) — never lazily mid-stage. Declared on `custom_fields` rather
  than a dedicated field so the capability doesn't force every `AgentConfig`
  struct literal across the workspace to grow it. A malformed declaration is
  its own typed `StageError::InvalidSkillsDeclaration { node, source }`, not
  flattened into `StageError::Internal(String)`.
- **`engine::stage::skill_binding::bind_skills`** — resolves each declared
  skill against the node's discovered `SkillCatalog` and, once bound, hands
  the caller each skill's resolved instructions
  (`skill_binding::BoundSkill::instructions`) so `engine::stage::agent`
  appends them to the agent's system prompt at session-open time — the
  content actually reaches the agent, not just the event log.
- **Trust gate delivered through the rendered path.** An unpinned, unhashed,
  or content-changed skill gates behind an operator decision emitted as
  `EventPayload::HumanInputRequested` / `HumanInputResolved` /
  `HumanInputTimedOut` — the same events `cockpit::dispatch::decide_action`
  and `surge inbox` already render — resolved through the engine's existing
  node-keyed `gate_resolutions` registry and `Engine::resolve_human_input`.
  The sender is registered into that registry only at the moment a prompt is
  actually requested, never eagerly at stage entry, so a node whose skills
  all bind without approval never touches the registry, and a prompt can
  never be resolved against a stale, already-abandoned entry. A denial or an
  unanswered prompt is `StageError::SkillApprovalRejected` — the node does
  not start, and nothing is bound for it.
- **`ApprovalConfig::skill_approval`** now gates real behavior (previously
  read nowhere): default enabled, so a profile that never mentions it keeps
  the trust check active; set to `false` to bind every declared skill on
  that node without a prompt. Every `SkillBound` event records
  `gate_enabled: bool` so a disabled gate is visible in the log, not silent.
- **`EventPayload::SkillBound`** carries `node: NodeKey` explicitly (not
  positional "nearest preceding `StageEntered`" inference), so a single
  event reconstructs the full "node → skill → hash" triple by itself — the
  event log alone is enough to know which skill, at which content, entered
  which node (R17).
- **`default_skill_roots`** scans `.claude/plugins` alongside `.claude/skills`
  under both the worktree and the user's home directory. The Agent Plugins
  corpus measured on a real machine (352 `SKILL.md`, 47
  `.claude-plugin/plugin.json` manifests) lives under `~/.claude/plugins`,
  not `~/.claude/skills` — scanning only the latter would never bind a
  single one of those 47 packages.
  See [ADR 0015](docs/adr/0015-skill-binding-trust-via-content-hash.md).
- **Event schema v6** (`surge_core::migrations::MAX_SUPPORTED_VERSION`) adds
  the `SkillBound` variant, carrying `node`, `name`, `provider`, `hash`, and
  `gate_enabled`. **Not "purely additive"**: an unknown enum variant has no
  representation for an old reader to fall back on at all (unlike an
  optional field, which can default) — the version is bumped precisely so a
  v5-max reader fails closed with a clean `SurgeError::SchemaTooNew` instead
  of an opaque unknown-variant decode error, the same reasoning as the
  v2/v4/v5 bumps before it. See
  [docs/schema-versioning.md](docs/schema-versioning.md#event-payloads-run-log).
- **Ambiguous unpinned names reach the operator, not a hard failure.**
  Measured on a real `~/.claude/plugins` corpus: 49 of 99 skill names have
  more than one physically distinct pack (up to five per name). An unpinned
  declaration of one of those names used to fail
  `StageError::SkillResolutionFailed` before the node ever asked anyone —
  roughly half the corpus, silently un-bindable. `bind_skills` now resolves
  a `hash: Some(pin)` declaration directly against that pin (narrowing to
  exact content — never ambiguous, per Решение §4) and only falls back to a
  name/provider(/version) lookup — where `SkillError::Ambiguous` is expected
  corpus shape, not a failure — when the pin doesn't match anything current.
  The resulting approval prompt names every candidate's hash; the
  lexicographically smallest is the one that binds if approved (a stable,
  deterministic tie-break, not a guess).
- **Skill catalog discovery moved off the async worker thread and is cached
  per run.** `SkillCatalog::discover` synchronously walks and hashes every
  pack under (now four) roots — hundreds of packs on a real corpus — and was
  being called inline from an async fn on every single agent node that
  declares skills. It now runs via `tokio::task::spawn_blocking` and is
  discovered at most once per run (cached on the run's execution state),
  reused by every later node instead of re-walking the filesystem each time.
- **`ApprovalConfig::skill_approval` also honors the resolved profile**, not
  only a node's own `approvals_override` — the same node-then-profile
  precedence `engine::stage::agent::effective_approvals` already establishes
  for every other approval flag.
- **Declared skills validated at graph load**, not lazily the first time a
  node's stage runs: `surge_core::validation::validate` now calls
  `AgentConfig::declared_skills()` on every agent node (top-level and inside
  subgraphs) and reports a malformed one as
  `ValidationErrorKind::InvalidSkillsDeclaration { node, reason }` — the run
  fails before `PipelineMaterialized`, before a worktree exists, matching
  the "a broken declaration does not fail the run silently" standard History
  15 already set for a broken `SKILL.md` (R09.1).
- **`AgentConfig::declared_skills()` is `#[must_use]`.**

### Added — Memory as claims: per-entry provenance and confidence

- **`surge_core::memory`** — a memory entry is now a claim, not free text:
  `MemoryClaim { id, text, provenance, confidence, status }`, with
  `Provenance { source, hash, verified_by, verified_at }` and a three-level
  `Confidence` (`verified` / `name_matched` / `asserted` — never a bool).
  `ClaimStatus::Verified` requires both `verified_by` and `verified_at` to
  be set; `MemoryClaim::new` rejects the combination otherwise
  (`UnprovenVerifiedStatus`). Anything ingested from a transcript or
  conversation starts `unverified` at capture time (`MemoryClaim::from_transcript`).
- **Memory DB schema bumped v1 → v2** (`surge_persistence::memory::schema::SCHEMA_VERSION`)
  — adds the `memory_claims` table. Opening a v1 database backfills every
  existing discovery/pattern/gotcha/file-context row into an unverified,
  `Asserted`-confidence claim in one atomic migration step; the v1 tables
  and their rows are left in place. See
  [docs/schema-versioning.md](docs/schema-versioning.md#memory-db-surge-persistence).

### Added — Crash recovery (v0.1 blocker)

- **New `surge-daemon::recovery` module** — daemon startup brings runs the
  registry left non-terminal back to life from the event log. A pure
  `decide_action` policy (skip-active / skip-terminal / reconcile-terminal /
  mark-failed-worktree-lost / flag-stuck / resume), a read-only
  `plan_recovery` registry scan, and an `execute_recovery` executor behind a
  `RecoveryEffects` trait. Model + decision table in
  [docs/crash-recovery.md](docs/crash-recovery.md).
- **Resumes share the live machinery** — recovered runs flow through the
  daemon's admission controller and broadcast registry via the new
  `server::resume_run_tracked` seam, so they publish `RunFinished` globally
  (tracker-completion comments and the L3 merge gate keep working). The
  admission controller is now created in `main.rs` and passed into
  `run_with_registry`.
- **`surge daemon recover [--dry-run]`** — read-only inspector by default;
  the plain form applies registry-safe reconciliations only while the daemon
  is stopped and reports resumes as pending the next `surge daemon start`.
- **`Storage::set_run_status`** for failed/terminal reconciliation.
- **Stuck-run detection** (>24h idle → human-attention card), **worktree
  consistency check** (lost worktree → `Failed`), per-stage recovery
  telemetry, and idempotent re-runs.

### Added — v0.1 release hardening

- **`surge --version`** embeds the git short SHA + commit date
  (`surge 0.1.0 (<sha>, <date>)`) via a new `surge-cli` build script.
- **Panic crash-report hook** — prints version + issue URL before the
  backtrace.
- **Schema freeze at v1** — `surge.toml` gains an optional `schema_version`
  (defaults to 1; `validate()` rejects other versions with a migration
  hint); `flow.toml` and event payloads were already at v1. Documented in
  [docs/schema-versioning.md](docs/schema-versioning.md).
- **License compliance** — `deny.toml` allow-list + `cargo deny` job in
  `security.yml`; [THIRD_PARTY.md](THIRD_PARTY.md) inventory.
- **CI MSRV (1.85) job** added to `ci.yml`.
- **v0.1 release notes draft** ([docs/release-notes-v0.1.md](docs/release-notes-v0.1.md))
  incl. zero-by-default telemetry posture.

### Added — Replay CLI mirror

- **`surge engine replay <run_id> --seq <N>`** folds the event log to seq
  `N` and prints the run state — CLI mirror of the replay scrubber over the
  same `aggregate_status` primitive.

### Fixed

- **Open-ended `RunReader::read_events`** — `EventSeq(u64::MAX)` was cast to
  `-1` for the SQLite bind, so `seq < -1` returned zero rows. Every
  open-ended read silently came back empty, which also degraded
  `current_status` (cockpit `/status`) and crash-recovery stuck/reconcile
  detection. Bounds are now clamped with `i64::try_from(..).unwrap_or(i64::MAX)`.

### Added — Telegram cockpit production-ready

- **New `surge-telegram` crate** — owns the long-running Telegram cockpit:
  bot loop scaffolding (`cockpit::run`), callback router
  (`cockpit::callback`), card renderer + emitter (`card::{render, emit}`),
  recovery reconciler (`cockpit::recover`), token-bucket rate limiter
  (`rate_limiter`), and production trait adapters (`cockpit::production`)
  that wrap persistence / engine / teloxide for daemon use. Crate split
  rationale documented in [ADR 0012](docs/adr/0012-surge-telegram-crate-split.md).
- **`surge telegram setup / revoke / list` CLI** (`surge-cli`) — persists
  the bot token under `telegram.cockpit.bot_token` in the registry
  SQLite, mints a 6-character base32 pairing token (default 10-minute
  TTL), and manages the `telegram_pairings` allowlist.
- **Paired-chat admission** — every callback and command runs an
  allowlist check against `telegram_pairings`; unpaired chats short-
  circuit before any engine call. `/pair` is the only command available
  to unpaired chats.
- **Cockpit cards** — `human_gate`, `bootstrap_<stage>`, `status`,
  `completion`, `failure`, `escalation`. Single card per
  `(run_id, node_key, attempt_index)` triple; updates use
  `editMessageText` only (no spam-sends). Hash-match short-circuits
  no-op updates ([ADR 0011](docs/adr/0011-telegram-card-lifecycle.md)).
- **Bot commands** — `/pair`, `/status`, `/runs`, `/run`, `/abort`,
  `/snooze`, `/feedback`. The mutating four (`/run`, `/abort`,
  `/snooze`, `/feedback`) reuse the same `Engine::resolve_human_input`
  contract the CLI console approvals use ([ADR 0009](docs/adr/0009-no-human-input-resolver-trait.md)).
- **Callback wire format** —
  [ADR 0010](docs/adr/0010-telegram-callback-schema.md) fixes the
  `cockpit:<verb>:<card_id>` schema; the inbox subsystem's
  `inbox:*` namespace remains untouched.
- **Snooze re-emission** — `/snooze 30m` on a reply to a cockpit card
  inserts into `inbox_action_queue` with `subject_kind='cockpit_card'`;
  the new `CockpitSnoozeRescheduler` polls due rows and edits the
  card with a `🛏 Snooze ended` footer once the wake-up time elapses.
  Snooze persistence extension to the inbox queue (registry migration
  0010) is shared with the existing inbox-ticket snooze pipeline.
- **Rate limiter** — token-bucket per chat (1/sec sustained, burst 5)
  plus a 25/sec global ceiling. Telegram `Retry-After` honoured
  verbatim with a single in-band retry.
- **Recovery reconciler** — on daemon restart and on tap-receiver
  `RecvError::Lagged`, walks every open card, joins against the
  current run snapshot, and either closes (terminal state) or edits
  (state diverged). Never issues a new `sendMessage`.
- **Daemon wiring** — `spawn_telegram_cockpit` helper in
  `surge-daemon` constructs all production adapters and spawns the
  cockpit runtime + snooze rescheduler next to the existing
  `TgInboxBot`. Cockpit failures absorbed by the outer loop's
  shutdown-aware `select!` so the rest of the daemon survives.
- **ADRs**: [0009](docs/adr/0009-no-human-input-resolver-trait.md),
  [0010](docs/adr/0010-telegram-callback-schema.md),
  [0011](docs/adr/0011-telegram-card-lifecycle.md),
  [0012](docs/adr/0012-surge-telegram-crate-split.md).
- **Docs**: [docs/telegram.md](docs/telegram.md) (new) + updates to
  `docs/cli.md`, `docs/workflow.md`, `docs/bootstrap.md`, `docs/README.md`.

Deliberately deferred (deferred-with-fallback per the milestone plan):
the **live polling update stream** (engine-tap dispatch and snooze
re-emit run in production today; callback delivery requires the
production `polling_default(bot)` adapter swap), the **webhook
receiver** (long-poll is the default), and the **sandbox-elevation
card** (owned by the sandbox-delegation-matrix milestone).

### Added — Tracker automation tiers (L0–L3)

- L0 / L1 / L2 / L3 tier model resolved deterministically from
  tracker labels (`surge:disabled`, `surge:enabled`,
  `surge:template/<name>`, `surge:auto`).
  [ADR 0013](docs/adr/0013-tracker-automation-tiers.md) and
  [docs/tracker-automation.md](docs/tracker-automation.md).
- `surge_intake::policy::resolve_policy` is the single resolver;
  proptests cover precedence under permutation.
- `surge intake list` CLI renders the ticket index (table or JSON).
- New persistence: `intake_emit_log` table for side-effect
  idempotency, `policy_hint` column on `inbox_action_queue` for L2
  carry, `CockpitSnoozeRescheduler` queue rows reuse the same table.
- `AutomationMergeGate` consumer evaluates L3 PR readiness on
  `RunFinished { Completed }` (the readiness check itself is stubbed
  to "Blocked: not yet implemented" — the gate's plumbing is
  complete; the GitHub-checks/review query lands in a follow-up).
- `CadenceController` algorithm ships in `surge_intake::cadence`;
  the integration into source poll loops is staged for a follow-up.

### Added — Self-describing artifact contracts

- **`surge artifact schema <kind>` CLI** — exports the JSON Schema (draft
  2020-12) describing on-disk artifacts. Supports `--all` to dump every
  kind in one object, `--format pretty|json`, and `--output <path>` to
  write to disk. Markdown-only kinds exit with an explanatory error
  listing required `## <Section>` headings instead of fabricating a
  schema.
- **`surge_core::json_schema_for` / `contract_summary` / `markdown_outline`**
  — pure introspection API that agents and external tools can call without
  shelling out. Every exported schema carries `$id`
  (`https://surge.dev/schema/v1/<artifact>.json`) and
  `x-surge-schema-version` for change tracking.
- **`#[derive(JsonSchema)]` across TOML artifact types** — `SpecArtifact`,
  `Spec`, `Subtask`, `AcceptanceCriteria`, `Complexity`, `SubtaskExecution`,
  `SubtaskState`, `RoadmapArtifact`, `RoadmapMilestone`, `RoadmapTask`,
  `RoadmapDependency`, `RoadmapRisk`, `RoadmapStatus`, `RoadmapPatch`,
  `RoadmapPatchOperation`, `RoadmapPatchItem`, `InsertionPoint`,
  `RoadmapItemRef`, and the rest of the roadmap-patch enum tree now derive
  JSON Schema. Custom-serde newtypes (`SpecId`, `SubtaskId`, `RunId`,
  `NodeKey` family, `ContentHash`, `RoadmapPatchId`) carry hand-written
  `JsonSchema` impls describing them as strings with the appropriate
  pattern/length constraints. On-disk serialization is unchanged.
- **Snapshot tests for every exported schema** in
  `crates/surge-core/tests/artifact_schema_snapshots.rs`. Breaking changes
  to a contract now surface as a diff that prompt profiles, IDE plugins,
  and external validators can track in lockstep.
- **`docs/artifact-schemas.md`** — quick-start, coverage matrix, and
  recommended consumption pattern for agents.

### Changed — Artifact contract acceptance-criteria validation

- **Stricter acceptance-criteria checks** — Spec (Markdown and TOML) and Story
  artifacts now reject placeholder, empty-checkbox, or too-short acceptance
  criteria (e.g. `TBD`, `- [ ]`, `?`). The new
  `empty_acceptance_criteria` diagnostic code points to the offending criterion
  by index (`Acceptance Criteria[N]` for Markdown,
  `spec.subtasks[i].acceptance_criteria[j]` for TOML). Each criterion must be a
  non-placeholder string of at least 8 characters after trimming.

### Added — Bootstrap & adaptive flow generation

- **`surge bootstrap` CLI** — runs the bundled Description Author →
  Roadmap Planner → Flow Generator bootstrap graph from a free-form prompt,
  supports console approve/edit/reject gates, resumes completed bootstrap
  runs via `surge bootstrap resume <run_id>`, and starts the materialized
  follow-up graph with inherited bootstrap artifacts.
- **Bundled flow registry and templates** — `surge-core::BundledFlows`
  embeds `bootstrap`, `linear-3`, `linear-with-review`, `multi-milestone`,
  `bug-fix`, `refactor`, `spike`, and `single-task`; `surge engine run
  --template <name>` resolves bundled or `${SURGE_HOME}/templates/*.toml`
  user templates and skips bootstrap.
- **Bootstrap runtime semantics** — bootstrap HumanGates emit structured
  approval/edit events, `EdgeKind::Backtrack` re-enters authoring agents
  with fresh `edit_feedback`, Flow Generator output retries on parse or
  graph-validation failures, and edit-loop caps emit `EscalationRequested`
  before failing clearly.
- **Content-addressed bootstrap artifacts** — agent-produced artifacts are
  copied into the per-run artifact store by content hash, and follow-up runs
  can inherit `description`, `roadmap`, and `flow` artifacts from their
  bootstrap parent via `EngineRunConfig::bootstrap_parent`.
- **Bootstrap telemetry and e2e coverage** — successful bootstrap runs append
  `BootstrapTelemetry` with stage durations, edit counts, and archetype
  metadata; mock-agent tests cover `linear-3`, `multi-milestone`,
  `bug-fix`, `refactor`, `spike`, validation retry, and edit-loop cap paths.
- **Documentation** — added [`docs/bootstrap.md`](docs/bootstrap.md) and
  refreshed workflow, CLI, getting-started, and archetype docs for bootstrap
  and template-skip usage.

### Added — Profile registry & bundled roles

- **`surge-core::profile::registry`** — pure inheritance + merge resolver.
  `ResolvedProfile`, `Provenance` (`Versioned` / `Latest` / `Bundled`),
  `merge_chain`, `merge_pair`, and a `collect_chain` walker with
  cycle detection and `MAX_EXTENDS_DEPTH = 8` guard. Shallow merge
  semantics codified per field shape: `default_mcp` / `default_skills` /
  `default_shell_allowlist` (each replaced when child non-empty),
  `bindings.expected` (merged by `name` field), `hooks.entries` (union
  dedup by `Hook::id` with WARN on collision), `prompt.system`
  (child wins when non-empty), `inspector_ui.fields`, `sandbox`,
  `approvals`. 32 unit tests + 8 property/snapshot tests.
- **`surge-core::profile::bundled`** — `BundledRegistry` with 17
  first-party profiles compiled in via `include_str!`: 3 bootstrap
  (Description Author, Roadmap Planner, Flow Generator), 7 execution
  (Spec Author, Architect, Implementer, Test Author, Verifier,
  Reviewer, PR Composer), 4 specialized via `extends`
  (Bug-Fix / Refactor Implementer, Security Reviewer, Migration
  Implementer), 2 project-level (Project Context Author, Feature
  Planner), and `mock@1.0` for tests.
- **`surge-core::profile::keyref`** — `ProfileKeyRef { name, version }`
  parser for `name@MAJOR.MINOR[.PATCH]` references. Partial versions
  zero-fill; double `@` and unparseable versions reject.
- **`surge-core` `RuntimeCfg::agent_id`** — new `String` field with
  `#[serde(default = "default_agent_id")]` returning `"claude-code"`.
  Identifies the agent runtime to launch via `surge_acp::Registry`
  lookup; replaces the M5 string-based fallback.
- **`surge-orchestrator::profile_loader`** — disk-touching half of the
  registry. `surge_home()` / `profiles_dir()` honour `SURGE_HOME` (fall
  back to `dirs::home_dir().join(".surge")`); `DiskProfileSet::scan`
  walks `*.toml` flat and warn-and-skips per-file parse failures;
  `ProfileRegistry::{load, resolve, list}` does the canonical
  versioned → latest → bundled 3-way lookup with version match
  against `Profile.role.version` in the TOML body.
- **`surge-orchestrator::prompt::PromptRenderer`** — Handlebars 6
  wrapper with strict mode (used at `ProfileRegistry::load` to fail
  loudly on broken templates) and lenient mode (used at agent stage
  execution to forgive missing optional bindings). HTML escaping
  disabled. `validate_template` is the load-time fail-fast hook.
- **`Engine::new_full`** constructor + `EngineConfig::profile_registry:
  Option<Arc<ProfileRegistry>>` field. Legacy constructors keep
  working with `profile_registry = None` (mock-only fallback);
  production wiring (CLI / daemon) calls `ProfileRegistry::load()` at
  startup.
- **`AgentStageParams::profile_registry`** — agent stages resolve
  `agent_config.profile` through it to derive `AgentKind` from the
  merged profile's `runtime.agent_id`. The M5 `if profile_str ==
  "mock"` fallback at `agent.rs:126-137` is gone — `mock@1.0` is now
  a bundled profile resolved through the registry like everything
  else. Unknown agent ids surface as `StageError::Internal` rather
  than silently degrading to mock.
- **`surge profile` CLI** — four new subcommands:
  - `surge profile list [--format json]`
  - `surge profile show <name> [--version X.Y.Z] [--raw]`
  - `surge profile validate <path>`
  - `surge profile new <name> [--base BASE]`
- **Documentation.** [`docs/profile-authoring.md`](docs/profile-authoring.md)
  is the new authoring guide (schema, inheritance, Handlebars,
  outcomes, sandbox/approvals/hooks, versioning, CLI).
  [ADR 0001](docs/adr/0001-profile-registry-layout.md) records the
  layout decisions; [ADR 0002](docs/adr/0002-profile-trust-deferred.md)
  defers signature/trust to post-v0.1.

### Changed — Profile registry & bundled roles

- `SurgeError` is now `#[non_exhaustive]` and gains the registry
  error family: `ProfileNotFound`, `ProfileVersionMismatch`,
  `ProfileExtendsCycle`, `ProfileExtendsTooDeep`, `ProfileFieldConflict`,
  `InvalidProfileKey`.
- `surge-orchestrator::engine::stage::bindings::substitute_template` is
  now `#[deprecated]` in favour of `PromptRenderer`. Kept around for
  the two legacy unit tests; new code routes through Handlebars.
- `docs/ARCHITECTURE.md` § 6 (Profiles and roles) and
  `.ai-factory/ARCHITECTURE.md` folder map updated to reflect the
  three-crate layering split.

## [Unreleased] — Graph engine GA

### Added

- **Hook execution chain (Phase 1).** New `engine::hooks` module in
  `surge-orchestrator` exposes `HookExecutor`, `HookContext`, and `HookOutcome`.
  Hooks fire on `pre_tool_use`, `post_tool_use`, `on_outcome`, and `on_error`
  triggers. Pre-tool rejection sends a synthetic `ToolResultPayload::Error`
  reply and skips the dispatcher; `on_outcome` rejection drops the agent's
  outcome attempt and lets it retry until `AgentLimits::max_retries` is
  exhausted, then appends `StageFailed`; `on_error` suppression converts a
  stage failure into a declared outcome via a JSON stdout directive.
- **Schema-version migration registry (`surge-core::migrations`).**
  `migrate_payload(version, bytes) -> Result<EventPayload, SurgeError>` is
  invoked by `surge-persistence` on every read; `MigrationChain` currently
  holds the v1 identity migration. `SchemaTooOld` / `SchemaTooNew` typed
  errors surface unsupported versions.
- **`ReferenceResolver` validation extension.** `surge-core` exposes a new
  `ReferenceResolver` trait and `validate_with_resolver` entry point;
  `surge-orchestrator` adds `validate_for_m6_with_resolver` which surfaces
  `ProfileNotFound`, `TemplateNotFound`, and `NamedAgentNotFound` errors.
  The terminal-only smoke path keeps using the no-resolver `validate_for_m6`.
- **Replay-determinism property test.** `surge-core/tests/fold_determinism_proptest.rs`
  asserts `fold` is idempotent and that incremental `apply()` matches one-shot
  `fold()` byte-for-byte at every prefix.
- **Six new `flow.toml` archetype examples** under `examples/`:
  `flow_linear_3.toml`, `flow_single_loop.toml`, `flow_multi_milestone.toml`,
  `flow_bug_fix.toml`, `flow_refactor.toml`, `flow_spike.toml`. All validate
  through `validate_for_m6_with_resolver` against the bundled
  `implementer@1.0` profile placeholder.
- **Documentation:** [`docs/hooks.md`](docs/hooks.md) (lifecycle, matcher,
  failure-mode matrix, suppression directive, profile authoring example) and
  [`docs/archetypes.md`](docs/archetypes.md) (gallery with mermaid diagrams
  for every archetype). Both linked from the root README and `docs/README.md`.
  `docs/ARCHITECTURE.md` § 4 cross-links the Hooks section.
- **Criterion bench:** `crates/surge-orchestrator/benches/stage_transition.rs`
  measures `StageEntered → OutcomeReported → EdgeTraversed` for a synchronous
  Branch node. `P95_BUDGET_US` constant carries the per-transition wall-clock
  budget. CI gates the bench via a Linux-only `bench` job that builds the
  bench and runs
  `SURGE_STAGE_TRANSITION_BUDGET_CHECK=1 cargo bench -- --quick --save-baseline ci`
  so budget regressions and runtime panics cannot land silently. Full GA
  baseline is local:
  `cargo bench -p surge-orchestrator --bench stage_transition -- --save-baseline ga`.
- **Gated real-ACP smoke test:**
  `crates/surge-orchestrator/tests/real_acp_smoke.rs` opts in via
  `SURGE_REAL_ACP_BIN` and `SURGE_REAL_ACP_PROFILE` env vars, runs
  `examples/flow_minimal_agent.toml` through the engine against the real
  ACP child, and asserts both `RunCompleted` and at least one
  `TokensConsumed` event. Optional `SURGE_REAL_ACP_KIND` and
  `SURGE_REAL_ACP_ARGS` override launch inference for custom agents. Without
  the required env vars the test prints a `SKIPPED` banner and exits
  successfully. See
  [`docs/development.md`](docs/development.md#optional-real-agent-smoke-test).
- **Daemon-attached engine path completion (Phase 3).**
  `BroadcastRegistry::subscribe_eventual` parks a oneshot waiter that is
  resolved atomically by the next `register(run_id)` call — closing the
  race where `spawn_forward_task` could push events before a queued
  subscriber attached. The IPC `Subscribe` handler now uses this path
  when the run is in the FIFO admission queue, so `subscribe_to_run`
  remains valid across the queued→admitted transition without an
  explicit re-subscribe. Covered by
  `crates/surge-daemon/tests/daemon_queued_subscribe_test.rs`.
- **Engine-facade parity test.**
  `crates/surge-daemon/tests/daemon_parity_test.rs` runs
  `flow_terminal_only.toml` through both `LocalEngineFacade` and
  `DaemonEngineFacade`, normalises wall-clock fields, and asserts the
  two event sequences are identical.
- **Mock-bridge archetype smoke suite.**
  `crates/surge-orchestrator/tests/archetypes_mock_test.rs` boots the
  engine against `fixtures::mock_bridge::MockBridge` for every bundled
  `examples/flow_*.toml` archetype; the terminal-only flow is run to
  completion and the rest are asserted to start cleanly. Complements
  `crates/surge-cli/tests/examples_smoke.rs` (parser+validator) and
  `tests/real_acp_smoke.rs` (gated real-agent path).

### Fixed

- **`EngineRunEvent::Terminal` IPC serialisation.** The variant was a
  tuple `Terminal(RunOutcome)` while both `EngineRunEvent` and
  `RunOutcome` use `#[serde(tag = "kind")]`. Internally-tagged tuple
  variants flatten the inner object's fields into the outer enum,
  which produced two `kind` fields on the wire and tripped
  `serde_json` with `duplicate field "kind"` on read. Changed to
  `Terminal { outcome: RunOutcome }` (struct variant) so the inner
  enum's tag nests cleanly. All callers (orchestrator engine, daemon
  facade, CLI watch loop, daemon inbox state-sync, integration tests)
  updated to the new field-style pattern.
- **StopRun on a queued run leaked `BroadcastRegistry::waiters` entries**
  introduced by `subscribe_eventual`. When a client called
  `Subscribe(run_id)` while the run was still in the FIFO admission
  queue and then `StopRun(run_id)` cancelled it before admission, the
  parked oneshot sender stayed in `waiters[run_id]` forever and the
  per-connection `forward_queued_to_client` task hung indefinitely.
  `StopRun`'s queued branch now calls `broadcast.deregister(run_id)`
  to drop the parked senders — each waiter wakes with
  `Err(RecvError::Closed)` and exits cleanly. Regression test:
  `daemon_queued_subscribe_test.rs::subscribe_to_queued_then_stop_does_not_leak_waiter`.
- **Hook executor pipe-buffer deadlock + child-leak on timeout.** The
  `spawn_via_shell` helper called `child.wait().await` and only THEN
  drained stdout/stderr. With piped output, a hook that emitted more
  than the pipe buffer (≈64 KiB Linux, ≈4 KiB Windows) blocked on
  write and never exited, hanging the engine forever in the await.
  And on timeout the future was dropped without killing the child,
  leaving the hook process orphaned. Switched to
  `child.wait_with_output()` (concurrent stdout/stderr drain + wait)
  combined with `Command::kill_on_drop(true)` so the timeout path
  genuinely terminates the hook. Caught in PR #48 review.
- **`on_error` hooks were skipping the `HookExecuted` audit trail.**
  `run_on_error_hooks` discarded `HookOutcome::executed()` and only
  returned the resolved suppression key, contradicting the
  "every hook invocation appends `HookExecuted`" rule honoured by
  the pre/post_tool_use and on_outcome chains. The helper now
  returns `OnErrorResolution { outcome, records }`, and the engine
  call site in `run_task::execute` persists each record via
  `record_hook_executed` before consuming the outcome. Unit tests
  in `run_task::tests` updated to verify the audit invariant.
  Caught in PR #48 review.
- **`validate_for_m6_with_resolver` silently dropped non-resolver
  errors.** The orchestrator-level resolver validator filtered
  `surge_core::validate_with_resolver` findings to only the
  `Profile/Template/NamedAgent NotFound` kinds, ignoring every other
  `Severity::Error` finding (single-edge-per-outcome, reachability,
  terminal reachability, loop iterable, etc.). Graphs that passed
  the engine's own structural checks but violated `surge-core`'s
  broader rules were accepted as valid. The validator now propagates
  every `Severity::Error` finding, prefixing each with
  `[ref]` (resolver) or `[structural]` so callers can tell them
  apart. Caught in PR #48 review and uncovered an unreachable
  `failure` terminal in `examples/flow_linear_3.toml` — fail-edge
  `kind` switched from `escalate` to `forward` so the terminal is
  reachable via forward traversal (semantics unchanged, the
  `escalate` kind is for parallel error-handler flows, not normal
  fail-outcome routing).
- **`StorageError::MigrationFailed` distinguishes schema-migration
  failures from raw I/O / pool faults.** The persistence read path
  was mapping `migrate_payload` errors onto `StorageError::Pool`,
  which surfaced as a misleading "pool error" even though the pool
  was healthy. Added a dedicated variant so log filters, dashboards,
  and tests can discriminate without pattern-matching error message
  strings. Caught in PR #48 review.

### Changed

- **`real_acp_smoke.rs` promoted from env-contract scaffold to real
  opt-in driver.** The test now mutates the minimal-agent example with a
  smoke-specific prompt, swaps the engine's mock fallback for the selected
  real ACP launch kind, waits for run completion, and verifies the persisted
  event log contains `RunCompleted` plus `TokensConsumed`.

- **Replay determinism violation in `RunState::apply`.** The proptest above
  uncovered that `apply()` generated `SessionId::new()` on `RunStarted` and
  inside `advance_bootstrap_stage`, violating the project rule
  "no random IDs introduced inside a fold". Replaced with the new
  `SessionId::nil()` deterministic placeholder so replay produces identical
  state byte-for-byte.

### Changed

- **`HookTrigger` is now `#[non_exhaustive]`** to honour the project rule
  for closed enums that may grow.
- **`MatcherSpec::file_glob`** now matches via `glob::Pattern::matches_path`
  (M1 stub used substring-match).

### Migration notes

- New workspace dependency: `glob = "0.3"` (used by
  `MatcherSpec::file_glob` evaluation).
- New `SurgeError` variants: `SchemaTooOld { found, min }` and
  `SchemaTooNew { found, max }`. The error enum is `#[derive(thiserror::Error)]`,
  so existing consumers compile unchanged.
- Persistence event reader now selects `schema_version` from the events
  table; existing rows continue to round-trip through the v1 identity
  migration.
