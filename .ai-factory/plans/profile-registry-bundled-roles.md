# Implementation Plan: Profile registry & bundled roles

Branch: exciting-greider-81695b (worktree; no new branch created — work on the existing isolated checkout)
Created: 2026-05-07
Refined: 2026-05-07 (deep code-verification pass via /aif-improve)

## Settings
- Testing: yes
- Logging: verbose (DEBUG / INFO / WARN / ERROR — TRACE is not used in this codebase)
- Docs: yes  # mandatory docs checkpoint at the end

## Roadmap Linkage
Milestone: "Profile registry & bundled roles"
Rationale: Next unchecked milestone in `.ai-factory/ROADMAP.md`. Graph engine GA shipped 2026-05-07 and explicitly leaves a `TODO(M6): wire profile → binary path via a ProfileRegistry lookup` in `crates/surge-orchestrator/src/engine/stage/agent.rs:129` — this milestone fulfills that TODO.

## Architectural Decisions Locked Before Implementation

These are decisions taken at planning time so /aif-implement does not re-litigate them per task. Verified against actual code in the refinement pass.

1. **Layering split (codified in ADR 0001 — Task 1):**
   - Pure inheritance/merge logic + bundled asset access → `surge-core::profile::registry` and `surge-core::profile::bundled` (preserves the I/O-free leaf invariant from `.ai-factory/ARCHITECTURE.md`)
   - Disk I/O (`~/.surge/profiles/` walks) + 3-way resolution → `surge-orchestrator::profile_loader`
   - CLI surface → `surge-cli::commands::profile`
2. **Asset bundling: `include_str!`**, not `rust-embed`. ~17 small TOML files, compile-time inlining, zero runtime cost.
3. **Template engine: Handlebars** (per roadmap deliverable). Strict mode on. Lives in `surge-orchestrator`. Renders `Profile::prompt.system` (the actual field; `PromptTemplate` has only `system: String`).
4. **Trust/signature: deferred to post-v0.1** with an explicit ADR (Task 2).
5. **`SURGE_HOME` env var** overrides `~/.surge`; falls back to `dirs::home_dir().join(".surge")` matching `crates/surge-persistence/src/store.rs:167-171`.
6. **Resolution lookup order:** versioned (`name-MAJOR.MINOR.toml`) → latest (`name.toml`) → bundled fallback. **Version match is canonical against `Profile.role.version`** (semver in TOML body); filename is just a hint.
7. **Profile→AgentKind binding (Task 28):** add `agent_id: String` to `RuntimeCfg` referencing `surge_acp::RegistryEntry::id`. Engine resolves binary path via the existing agent registry. Default value `"claude-code"`. (Without this field there is no way to derive `AgentKind`; the previous draft missed this and would have shipped an unimplementable plan.)
8. **Engine wiring (Tasks 29, 16, 30):** `EngineConfig` gains `profile_registry: Option<Arc<ProfileRegistry>>`. New `Engine::new_full(...)` constructor; legacy constructors delegate. `AgentStageParams` gains `profile_registry` mirroring the existing `mcp_registry` pattern. Both `surge-cli` and `surge-daemon` entry points construct the registry once at startup.
9. **Mock preserved (Task 31):** the M5 special-case `if profile_str == "mock"` is removed; `mock@1.0` ships as a bundled profile so existing tests keep working unchanged.
10. **Merge semantics (shallow), corrected to actual field shapes:**
    - `runtime` scalars → child wins on non-default
    - `tools` (`default_mcp`, `default_skills`, `default_shell_allowlist` — three Vec<String> fields, NOT a single `allowed_tools`): child fully replaces parent for each
    - `outcomes` (Vec<ProfileOutcome>): child fully replaces parent
    - `bindings.expected` (Vec<ExpectedBinding>): merged by `name` field (child overrides match, parent's other entries preserved)
    - `hooks.entries` (Vec<Hook>): union dedup by `Hook::id`, child wins on collision (WARN-logged)
    - `prompt.system` (String): child wins when non-empty
    - `inspector_ui.fields` (Vec<InspectorUiField>): child fully replaces parent
    - `sandbox` (SandboxConfig containing SandboxMode `read_only|workspace_write|workspace_network|full_access|custom`): child wins as a whole
11. **Logging targets** follow the established `engine::*` / `profile::*` / `cli::*` naming (per `target: "engine::hooks"`, `target: "engine::stage::agent"` examples in the existing codebase). NOT `surge::*`. TRACE level is not used in this codebase — stick to DEBUG/INFO/WARN/ERROR.
12. **Clippy budget**: `clippy.toml` enforces cognitive-complexity ≤ 25, function length ≤ 100 lines, max 3 bool struct fields, max 7 fn args (3 bool args max). New modules must respect these limits — break long merge functions into helpers if needed.

## Commit Plan (8 commits)

- **Commit 1** (after tasks 1-3): `docs(profile): record registry layout decisions and add handlebars dep`
- **Commit 2** (after tasks 28, 4-7): `feat(core): add agent_id field, registry errors, and inheritance resolver with cycle detection`
- **Commit 3** (after tasks 8-12, 31): `feat(core): bundle 17 profile assets via include_str! (incl. mock)`
- **Commit 4** (after tasks 13-15, 29): `feat(orchestrator): disk loader + ProfileRegistry + EngineConfig wiring`
- **Commit 5** (after task 16): `feat(orchestrator): replace M5 mock fallback with registry-driven AgentKind resolution`
- **Commit 6** (after tasks 17-18): `feat(orchestrator): render system prompts via Handlebars with strict mode`
- **Commit 7** (after tasks 19-23): `feat(cli): add surge profile subcommand group (list/show/validate/new)`
- **Commit 8** (after tasks 30, 24-26, 32, 27): `chore: wire registry into binaries, ship integration test, docs, changelog`

## Tasks

### Phase 0 — Decisions (no code)

- [x] Task 1: ADR 0001 — profile registry layout. Files: `docs/adr/0001-profile-registry-layout.md`
- [x] Task 2: ADR 0002 — defer profile trust/signature to post-v0.1. Files: `docs/adr/0002-profile-trust-deferred.md`
- [x] Task 3: Add `handlebars = "6"` to workspace `[workspace.dependencies]` and `crates/surge-orchestrator/Cargo.toml`. Files: `Cargo.toml`, `crates/surge-orchestrator/Cargo.toml`
<!-- Commit checkpoint: tasks 1-3 -->

### Phase 1 — Pure registry primitives in `surge-core`

- [x] Task 28: Add `agent_id: String` to `RuntimeCfg` (default `"claude-code"`); add `parse_key_ref(s) -> ProfileKeyRef { name, version }` helper for `name@version` syntax (depends on 1)
- [x] Task 4: Add `ProfileRegistryError` family to `SurgeError` (`ProfileNotFound`, `ProfileVersionMismatch`, `ProfileExtendsCycle`, `ProfileExtendsTooDeep`, `ProfileFieldConflict`, `InvalidProfileKey`). Retrofit `#[non_exhaustive]`. Files: `crates/surge-core/src/error.rs`
- [x] Task 5: Implement `ResolvedProfile`, `Provenance`, `merge_chain` in `crates/surge-core/src/profile/registry.rs` using the corrected merge semantics for actual fields (`default_mcp`/`default_skills`/`default_shell_allowlist`, `bindings.expected`, `hooks.entries`, `prompt.system`) (depends on 1, 4, 28)
- [x] Task 6: `MAX_EXTENDS_DEPTH = 8`, `collect_chain` walker with cycle detection and depth guard (depends on 4, 5)
- [x] Task 7: Property tests via `proptest` + insta snapshots for representative resolved profiles (depends on 5, 6)
<!-- Commit checkpoint: tasks 28, 4-7 -->

### Phase 2 — Bundled assets in `surge-core`

- [x] Task 8: `BundledRegistry` skeleton with `include_str!` pattern; create `crates/surge-core/bundled/profiles/` dir; re-export from `lib.rs` (depends on 1)
- [x] Task 9: Author 3 bootstrap profiles — Description Author, Roadmap Planner, Flow Generator — using REAL field shape (`prompt = { system = "..." }`, `tools = { default_mcp = [], default_skills = [], default_shell_allowlist = [] }`, `sandbox = { mode = "read_only" }`, `runtime` includes `agent_id`). Register in `BundledRegistry::all()` (depends on 8, 28)
- [x] Task 10: Author 7 execution profiles — Spec Author, Architect, Implementer, Test Author, Verifier, Reviewer, PR Composer (depends on 8, 28)
- [x] Task 11: Author 4 specialized variants using `extends` (Bug-Fix, Refactor, Security Reviewer, Migration Implementer) + assertion test that every bundled profile resolves through the merge chain (depends on 5, 6, 10)
- [x] Task 12: Author 2 project-level profiles — Project Context Author, Feature Planner (depends on 8, 28)
- [x] Task 31: Bundle `mock@1.0.toml` so the M5 mock test path keeps working through the registry (`runtime.agent_id = "mock"` resolves to `AgentKind::Mock`) (depends on 8, 28)
<!-- Commit checkpoint: tasks 8-12, 31 -->

### Phase 3 — Disk loader, ProfileRegistry, engine plumbing

- [x] Task 13: `surge_home()` and `profiles_dir()` helpers honoring `SURGE_HOME`; mirror `surge-persistence::store::default_path` pattern. Files: `crates/surge-orchestrator/src/profile_loader/{mod,paths}.rs`
- [x] Task 14: `DiskProfileSet::scan` walking `*.toml` flat under `profiles_dir()`; warn-and-skip on parse failure; tempdir tests (depends on 13)
- [x] Task 15: `ProfileRegistry::{load, resolve, list}` with 3-way lookup matching `Profile.role.version` (canonical) + `Provenance` tagging (depends on 8, 14, 5, 6)
- [x] Task 29: Add `profile_registry: Option<Arc<ProfileRegistry>>` to `EngineConfig`; new `Engine::new_full(...)` constructor; legacy constructors delegate (depends on 15)
- [x] Task 16: Replace M5 fallback at `crates/surge-orchestrator/src/engine/stage/agent.rs:126-137` with registry-driven `AgentKind` derivation via `runtime.agent_id` → `surge_acp::Registry` lookup. Add `profile_registry` to `AgentStageParams` mirroring existing `mcp_registry` field (depends on 15, 17, 28, 29)
<!-- Commit checkpoint: tasks 13-15, 29 (one commit), then 16 (separate commit) -->

### Phase 4 — Prompt template engine

- [x] Task 17: `PromptRenderer` wrapper around `handlebars::Handlebars` (strict mode, no HTML escape); replaces `substitute_template` in `crates/surge-orchestrator/src/engine/stage/bindings.rs:87-97` and the call site at `agent.rs:124`. Renders `profile.prompt.system` (depends on 3)
- [x] Task 18: Validate every profile's `prompt.system` at `ProfileRegistry::load` time; fail-fast on bundled or disk template errors (depends on 15, 17)
<!-- Commit checkpoint: tasks 17-18 -->

### Phase 5 — CLI surface in `surge-cli`

- [x] Task 19: Scaffold `ProfileCommands` enum + `commands/profile.rs` + main.rs wiring; subcommand bodies `bail!()` until tasks 20-23 fill them (depends on 15)
- [x] Task 20: `surge profile list` with provenance column + optional `--format json` (depends on 19)
- [x] Task 21: `surge profile show <name> [--version X.Y.Z] [--raw]` rendering merged or raw profile as TOML (depends on 19, 17)
- [x] Task 22: `surge profile validate <path>` checking schema + `prompt.system` Handlebars syntax + extends parent existence (depends on 19, 17)
- [x] Task 23: `surge profile new <name> [--base BASE]` scaffolder; refuses to overwrite existing files (depends on 19)
<!-- Commit checkpoint: tasks 19-23 -->

### Phase 6 — Binary wiring, integration test, docs, acceptance

- [ ] Task 30: Wire `ProfileRegistry::load()` in `crates/surge-cli/src/commands/engine.rs` (after `tool_dispatcher` construction, before `Engine::new_full(...)`) and the matching site in `crates/surge-daemon/` (depends on 16, 29)
- [ ] Task 24: E2E integration test `crates/surge-orchestrator/tests/profile_registry_e2e.rs` — `tempdir + SURGE_HOME` override of bundled `implementer@1.0`, run minimal flow against mock agent, assert overridden prompt reaches the agent and `Provenance::Latest` is recorded (depends on 30, 31)
- [ ] Task 25: Authoring guide `docs/profile-authoring.md` covering schema, inheritance, Handlebars bindings (`prompt.system`), outcome contract, sandbox/approvals/hooks, versioning, validate/scaffold workflow (depends on 20, 21, 22, 23)
- [ ] Task 26: Update `docs/ARCHITECTURE.md` and `.ai-factory/ARCHITECTURE.md` with the new layering (depends on 16)
- [ ] Task 27: Acceptance gate — `cargo build --workspace` clean, `cargo test --workspace` green, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo doc --workspace --no-deps` clean, plus roadmap deliverable cross-check ✅ (depends on 24, 25, 26)
- [ ] Task 32: Update `CHANGELOG.md` `[Unreleased]` section with all milestone entries (depends on 27)
<!-- Commit checkpoint: tasks 30, 24-26, 27, 32 -->

## Out of Scope (explicitly deferred)

- **Remote profile download / fetch** — bundled + disk only in v0.1.
- **Profile signature verification** — see ADR 0002.
- **`--force` overwrite for `surge profile new`** — refusing overwrite is the safer default.
- **Trust/sandbox model for user-authored hooks inside profiles** — orchestrator already mediates hooks.
- **Profile package format / sharing mechanism** — independent design discussion.
- **Roadmap milestone "Bootstrap & adaptive flow generation" consumer wiring** of the bundled bootstrap profiles — lives in the next milestone's plan.
- **TRACE-level logging of bindings** — codebase doesn't use TRACE; if needed later, add it then.

## Logging Conventions

All new modules emit via `tracing::*` macros (no `println!`, no `dbg!` per architecture anti-patterns). Targets follow the existing `engine::*` / `profile::*` / `cli::*` `::`-separated naming convention seen in `crates/surge-orchestrator/src/engine/hooks/mod.rs`.

- **Targets used in this milestone:**
  - `profile::merge` (surge-core inheritance resolver)
  - `profile::chain` (surge-core extends walker)
  - `profile::keyref` (surge-core key parser)
  - `profile::bundled` (surge-core BundledRegistry)
  - `profile::registry` (surge-orchestrator ProfileRegistry)
  - `profile::disk` (surge-orchestrator disk loader)
  - `profile::paths` (surge-orchestrator path resolver)
  - `profile::validate` (template validation)
  - `engine::stage::agent` (existing — extended with new fields)
  - `engine::prompt` (Handlebars rendering)
  - `engine::startup` (binary entry-point wiring)
  - `cli::profile::{list,show,validate,new}`
- **Levels:**
  - `INFO` — registry constructed, scan completed, override decisions visible to operator
  - `DEBUG` — per-resolve, per-merge, per-render outcomes (verbose preference)
  - `WARN` — recoverable degradations (malformed profile skipped, hook id collision, duplicate (id, version) on disk)
  - `ERROR` — load failures, render failures, unknown-profile resolution

## Refinement Summary (2026-05-07)

Deep code-verification pass found 6 foundational mismatches between the v1 plan and actual code:

| v1 assumption | Reality | Fix |
|---|---|---|
| `RuntimeCfg.launch_kind` field exists | Does not exist; no profile→binary mapping at all | Task 28: add `agent_id: String` |
| `ToolsCfg.allowed_tools` | Three Vec fields: `default_mcp`, `default_skills`, `default_shell_allowlist` | Task 5 merge semantics rewritten |
| `PromptTemplate.template` | Field is `system: String` | Tasks 17/18/22 corrected |
| `ProfileKey` parses `name@version` | Just an allowed character; no parser | Task 28: `parse_key_ref` helper |
| `EngineCtx` exists for stage params | Stages take explicit `*StageParams` structs | Task 16: mirror `mcp_registry` pattern in `AgentStageParams` |
| Single engine constructor | Three constructors (`new` / `_with_notifier` / `_with_mcp`) | Task 29: add `Engine::new_full` |

5 new tasks added (28-32), 10 existing tasks rewritten (5, 9-12, 15, 16, 17, 18, 22), dependencies adjusted across phases. Commit plan extended from 7 to 8 commits.
