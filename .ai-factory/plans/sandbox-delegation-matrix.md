# Implementation Plan: Sandbox Delegation Matrix

Branch: objective-mcnulty-9cd45f (planning worktree; no new feature branch created)
Created: 2026-05-12

## Settings
- Testing: yes (unit + integration; criterion bench unchanged)
- Logging: verbose (`tracing::debug!` on every sandbox decision and elevation hop; `info!` for resolution result; `warn!` for unsupported combos; `error!` only on real failures)
- Docs: yes (mandatory `/aif-docs` checkpoint at completion — sandbox matrix table is a public-facing contract)

## Roadmap Linkage
Milestone: "Sandbox delegation matrix"
Rationale: Next unchecked roadmap milestone; follow-up to ADR-0006 (ACP-only) — locks the runtime mapping ACP-only depends on. Also resolves RESEARCH.md Active Summary Open Question #1 (widen runtime set to current ACP-subscription-CLI top tier).

## Research Context
Source: `.ai-factory/RESEARCH.md` (Active Summary, 2026-05-11)

Goal: Make `SandboxMode` (named `SandboxIntent` in roadmap text — keep the existing `SandboxMode` identifier; "intent" is the conceptual term in docs) a real contract — every supported runtime maps every variant to native CLI flags, unsupported combos refuse to run, the ACP elevation roundtrip is wired end-to-end, and a `surge doctor` command exposes the matrix.

Constraints carried over from Active Summary:
- ACP-only transport (ADR-0006). No non-ACP fallback parsers.
- Subscription-CLI scope: Claude Code, Codex CLI, Gemini CLI are v0.1 blockers. Cursor CLI, Copilot CLI (public preview), OpenCode are stretch — wire infra so adding them is one registry edit + one mapping table row, not a new code path.
- Adapter quality variance is accepted (three-layer debug surface): native vs adapter agents.
- `unstable_session_usage` is pinned at agent-client-protocol v0.10.2 — do not chase latest during this milestone.

Decisions:
1. Keep the existing `SandboxMode` identifier in `surge-core::sandbox` (already shipped in events / configs). Roadmap text saying `SandboxIntent` is conceptual; do not rename.
2. Matrix lives as **data**, not code: a `RuntimeSandboxMatrix` table in `surge-core` keyed by `(runtime_kind, SandboxMode)`. Per-runtime adapters consume the table; new runtimes are a registry entry + a row, not a new module.
3. Custom mode (`SandboxConfig { mode: Custom, ... }`) → pass-through validated launch flags. Validator lives in `surge-core` (per `feedback_spec_scope_discipline.md`: validation in core).
4. Unsupported `(mode × runtime)` combos refuse to start the run. No silent downgrade. `surge doctor` surfaces the same matrix with hint text.
5. Elevation roundtrip uses the existing `SandboxElevationRequested` / `SandboxElevationDecided` events (already declared in `crates/surge-core/src/run_event.rs:262-271`; `ElevationDecision { Allow, AllowAndRemember, Deny }` already exists). The ACP `RequestPermissionRequest` callback inside `surge-acp::bridge::session` / `bridge::client` translates into a `SandboxElevationRequested` event; the orchestrator routes through `surge-notify` approval channels; the decision is fed back to the SDK via `RequestPermissionResponse`.
6. Per-runtime minimum versions are declared as data in the same registry (e.g., `min_version = "0.5.0"`); `surge doctor` reports stale binaries.
7. Retrofit `#[non_exhaustive]` on new public types from day one AND on the existing `SandboxMode` enum (carried-over rule from `feedback_spec_scope_discipline.md`; folded into Task 1).
8. Timeout-as-Deny is recorded via a NEW `SandboxElevationTimedOut { node, capability }` event variant (rather than extending the existing `SandboxElevationDecided` payload shape). The existing payload has `node`, `decision`, `remember` and no `reason` field — adding a separate audit event preserves backward compat and survives replay cleanly through the schema migration in Task 8b.
9. `RuntimeKind` does NOT carry a `Custom` variant (the existing `SandboxConfig::Custom` already covers that axis) and does NOT yet carry `Junie` / `Augment` variants — those land when their first matrix row lands, per decide-or-defer.

Open questions resolved during planning:
- Cursor / Copilot / OpenCode / Goose / Junie / Augment — wire matrix infra to support them, but **only Claude Code + Codex + Gemini ship verified rows in v0.1**. Others land as "declared, unverified" rows; `surge doctor` flags them. This is the "decide-or-defer" half (we decide infra, defer verification).
- Audit logging — every elevation request + decision already produces an event; no separate audit log needed. Documentation task covers it.
- `surge doctor agent <name>` smoke session: reuse mock-agent infra from `surge-acp` tests; do not invent a new harness.

Open questions deferred (out of scope for this milestone, captured in follow-up):
- Trust / signature story for shared profiles (separate `Profile registry` polish).
- `--trace-acp` flag — split into its own ticket; touches every ACP call site.

## Commit Plan
- **Commit 1** (after tasks 1-3): `feat(surge-core): add RuntimeSandboxMatrix, minimum-version registry, retrofit #[non_exhaustive]`
- **Commit 2** (after tasks 4-6, 6b): `feat(surge-acp): map SandboxMode to native flags for Claude/Codex/Gemini and wire registry runtime mapping`
- **Commit 3** (after tasks 7-8, 8b): `feat(surge-acp,surge-orchestrator,surge-core): wire ACP elevation roundtrip end-to-end with schema v2`
- **Commit 4** (after task 9): `test(surge): negative tests for blocked elevation and unsupported sandbox combos`
- **Commit 5** (after tasks 10-12): `feat(surge-cli): add surge doctor with sandbox matrix and version checks`
- **Commit 6** (after tasks 13-15): `test(surge): integration, property, and audit tests for elevation`
- **Commit 7** (after task 16): `docs: sandbox delegation matrix and elevation runbook`

## Tasks

### Phase 1: Core matrix types (surge-core, leaf)

- [x] **Task 1: Introduce `RuntimeKind` enum and matrix table in `surge-core`, retrofit `#[non_exhaustive]` on `SandboxMode`.**
  - Add `crates/surge-core/src/runtime.rs` with `pub enum RuntimeKind` — closed enum, `#[non_exhaustive]`, variants: `ClaudeCode`, `Codex`, `Gemini`, `CursorCli`, `CopilotCli`, `OpenCode`, `Goose`. NO `Custom` variant (`SandboxConfig::Custom` already covers that axis). NO `Junie` / `Augment` yet (decide-or-defer until matrix rows land). Derive `Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize`. Implement `Display` and a stable `as_str()` returning the snake_case wire form.
  - Retrofit `#[non_exhaustive]` onto the existing `SandboxMode` enum at `crates/surge-core/src/sandbox.rs:33` (per feedback_spec_scope_discipline.md). Adjust the few match arms that break (e.g., `crates/surge-orchestrator/src/engine/sandbox_factory.rs`) by adding a wildcard arm with a `// TODO: SandboxMode added — confirm matrix coverage` comment.
  - Add `crates/surge-core/src/sandbox_matrix.rs` with `#[non_exhaustive] pub struct RuntimeSandboxRow { pub runtime: RuntimeKind, pub mode: SandboxMode, pub flags: Vec<String>, pub env: BTreeMap<String, String>, pub verified: bool, pub min_version: Option<semver::VersionReq> }`.
  - Add `pub struct RuntimeSandboxMatrix(Vec<RuntimeSandboxRow>)` with `pub fn lookup(&self, runtime: RuntimeKind, mode: SandboxMode) -> Option<&RuntimeSandboxRow>`, `pub fn unsupported(&self, runtime: RuntimeKind, mode: SandboxMode) -> bool`, and `pub fn verified_only(&self) -> impl Iterator<Item = &RuntimeSandboxRow>`.
  - Add bundled default matrix as `pub fn default_matrix() -> RuntimeSandboxMatrix` populated from `crates/surge-core/bundled/sandbox/matrix.toml` (new) via `include_str!` + `toml::from_str`. Three verified rows (Claude/Codex/Gemini × 4 modes), declared-unverified rows for Cursor/Copilot/OpenCode/Goose.
  - `semver` is already at workspace deps version "1"; add `semver = { workspace = true }` to `crates/surge-core/Cargo.toml` if absent.
  - Re-export from `surge-core::lib`.
  - **Files:** `crates/surge-core/src/runtime.rs`, `crates/surge-core/src/sandbox_matrix.rs`, `crates/surge-core/src/sandbox.rs`, `crates/surge-core/src/lib.rs`, `crates/surge-core/bundled/sandbox/matrix.toml`, `crates/surge-core/Cargo.toml`.
  - **Logging:** none (leaf crate, no I/O). Tests use snapshots.
  - **Tests:** `#[cfg(test)] mod tests` in both new files. Property test with `proptest`: round-trip TOML serialization for every row; `insta` snapshot for the bundled matrix; deterministic `BTreeMap`/`Vec` ordering verified; assert no row has empty `flags` AND `verified=true`.

- [x] **Task 2: Validate `SandboxConfig::Custom` launch flags in core.**
  - Extend `crates/surge-core/src/sandbox.rs` with `pub fn validate_custom(cfg: &SandboxConfig) -> Result<(), SandboxValidationError>`. Rules: when `mode == Custom`, at least one of `writable_roots` / `network_allowlist` / `shell_allowlist` must be non-empty; `writable_roots` paths cannot escape with `..` segments; `network_allowlist` entries must parse as host/IP patterns; `shell_allowlist` cannot contain `;`, `|`, `&&` (per `feedback_spec_scope_discipline.md`: validation in core, not adapters).
  - Add `SandboxValidationError` variant to `SurgeError` (via `#[from]` or local enum re-surfaced through `validation.rs`); `#[non_exhaustive]`.
  - Hook validator at `Graph::validate()` so an invalid `SandboxConfig` on any `Agent` node fails graph load — not session open.
  - **Files:** `crates/surge-core/src/sandbox.rs`, `crates/surge-core/src/error.rs`, `crates/surge-core/src/validation.rs`.
  - **Logging:** none (leaf).
  - **Tests:** unit tests in `sandbox.rs` covering each rule; one happy + three failure cases per rule; deterministic error message via `insta`.

- [x] **Task 3: Add per-runtime minimum-version policy and `doctor` query surface.**
  - Add `pub struct RuntimeVersionPolicy { pub runtime: RuntimeKind, pub min_version: semver::VersionReq, pub note: String }` to `crates/surge-core/src/runtime.rs`.
  - Bundled policy in `bundled/sandbox/versions.toml` — Claude Code, Codex, Gemini at known minimums (use cached registry data; pick latest stable as of 2026-05; leave note explaining bump policy).
  - Add `pub fn version_policy(runtime: RuntimeKind) -> Option<&'static RuntimeVersionPolicy>` and a typed report builder (`pub struct DoctorReport { entries: Vec<DoctorEntry> }`, also `#[non_exhaustive]`).
  - **Files:** `crates/surge-core/src/runtime.rs`, `crates/surge-core/bundled/sandbox/versions.toml`, `crates/surge-core/src/doctor.rs` (new module for the `DoctorReport` types only — no I/O).
  - **Logging:** none (leaf).
  - **Tests:** snapshot test for bundled versions; `proptest` round-trip; `DoctorReport` ordering deterministic.
  - **Depends on Task 1.**

<!-- Commit checkpoint: tasks 1-3 → "feat(surge-core): add RuntimeSandboxMatrix and minimum-version registry" -->

### Phase 2: ACP adapter — per-runtime mapping

- [x] **Task 4: Replace `AlwaysAllowSandbox` stub with matrix-driven launch-flag resolver for Claude Code.**
  - New file `crates/surge-acp/src/bridge/sandbox_resolver.rs`: `pub fn resolve_launch_flags(runtime: RuntimeKind, cfg: &SandboxConfig, matrix: &RuntimeSandboxMatrix, ctx: ResolveContext) -> Result<Vec<String>, SandboxResolveError>`. `ResolveContext` distinguishes `Run` from `Doctor` callers so unverified rows are allowed under doctor but refused in production runs.
  - `SandboxResolveError` is `#[non_exhaustive]` with variants `UnsupportedCombo { runtime, mode }`, `UnverifiedRuntime { runtime }`, `CustomInvalid(SandboxValidationError)`.
  - For `RuntimeKind::ClaudeCode`: emit `--allow-tool=*` / `--deny-tool=*` style flags per mode (read-only ⇒ deny-write; workspace-write ⇒ allow-fs-write deny-network; workspace+network ⇒ allow-fs-write allow-network; full-access ⇒ `--dangerously-allow-all` plus a `warn!` log on resolve).
  - Update `crates/surge-orchestrator/src/engine/sandbox_factory.rs::build_sandbox()` to consume the resolver. Keep the `Sandbox` trait surface in `surge-acp::bridge::sandbox` unchanged — only the build path changes. Re-export `sandbox_resolver` from `surge-acp::bridge::mod`.
  - **Files:** `crates/surge-acp/src/bridge/sandbox_resolver.rs` (new), `crates/surge-acp/src/bridge/mod.rs`, `crates/surge-orchestrator/src/engine/sandbox_factory.rs`.
  - **Logging:** `tracing::debug!(target: "surge_acp.sandbox", runtime = ?runtime, mode = ?cfg.mode, flags = ?flags, "resolved sandbox launch flags");` on success. `tracing::warn!` once when emitting full-access flags. `tracing::error!(target: "surge_acp.sandbox", error = ?err, "sandbox resolve failed")` once in the error branch.
  - **Tests:** table-driven unit tests with `insta` snapshot per `(runtime, mode)` pair for Claude Code; one error-path test per `SandboxResolveError` variant.

- [x] **Task 5: Add matrix rows + resolver branches for Codex CLI and Gemini CLI (verified runtimes).**
  - Codex: map to `--sandbox=read-only` / `--sandbox=workspace-write` / `--sandbox=workspace+network` / `--sandbox=danger-full-access` (existing Codex sandbox model). Gemini: map to Gemini's native sandbox flags (`--sandbox=docker` is the only "real" mode it supports — for `read-only`/`workspace-write` emit a documented downgrade refusal *only when* the user did not set `mode=full-access`; full-access maps to no sandbox flag).
  - Encode the Gemini limitation as an explicit unsupported row in the bundled matrix (`flags = []`, `verified = false`, note explaining the gap) — refuses to run rather than silently downgrading.
  - **Files:** `crates/surge-core/bundled/sandbox/matrix.toml` (rows), `crates/surge-acp/src/bridge/sandbox_resolver.rs` (branches).
  - **Logging:** identical pattern to Task 4.
  - **Tests:** snapshot per pair; one negative test per refused combo verifying `SandboxResolveError::UnsupportedCombo { runtime, mode }`.
  - **Depends on Task 4.**

- [x] **Task 6: Declare (unverified) matrix rows for Cursor CLI, Copilot CLI, OpenCode, Goose.**
  - Populate `bundled/sandbox/matrix.toml` with `verified = false` rows and concrete native flags where the agent CLI documents them as of 2026-05; otherwise empty `flags = []` with a non-empty `note` field naming the upstream tracking issue / docs URL.
  - The resolver returns `SandboxResolveError::UnverifiedRuntime` unless `ResolveContext::Doctor` was passed (see Task 4). Decide-or-defer: declared infrastructure for Cursor/Copilot/OpenCode/Goose, not enforced execution.
  - DO NOT add `RuntimeKind::Junie` / `RuntimeKind::Augment` enum variants in this milestone — adding variants without matrix rows violates decide-or-defer. Captured in Out of Scope.
  - **Files:** `crates/surge-core/bundled/sandbox/matrix.toml`, `crates/surge-acp/src/bridge/sandbox_resolver.rs`.
  - **Logging:** `tracing::warn!(runtime = ?runtime, "matrix row unverified; refusing to start non-doctor run")`.
  - **Tests:** one refusal test per declared-unverified runtime in `ResolveContext::Run`; one acceptance test per runtime in `ResolveContext::Doctor`.
  - **Depends on Task 5.**

- [x] **Task 6b: Map detected agents to `RuntimeKind` in `builtin_registry.json`.**
  - Add a `runtime` field to every entry in `crates/surge-acp/builtin_registry.json` so `surge doctor` and the resolver can map a detected agent name to a `RuntimeKind` matrix row. Mapping: `claude-acp → claude-code`, `codex-acp → codex`, `gemini → gemini`, `github-copilot-cli → copilot-cli`, plus any other entries the registry lists today.
  - Update the parser/loader in `crates/surge-acp/src/registry.rs` (or whichever sibling owns the JSON schema) to read the new field as `Option<RuntimeKind>` with `#[serde(default)]`. Emit `tracing::warn!` on load when the field is absent — old registry files keep parsing but lose the matrix link.
  - Update `Registry::detect_installed_with_paths()` to surface the runtime alongside the binary path so `surge doctor` does not need a second lookup table.
  - **Files:** `crates/surge-acp/builtin_registry.json`, `crates/surge-acp/src/registry.rs`.
  - **Logging:** `tracing::debug!(target: "surge_acp.registry", agent, runtime, "registry entry loaded with runtime")`; `tracing::warn!(target: "surge_acp.registry", agent, "registry entry missing runtime field; matrix lookup will be skipped")`.
  - **Tests:** parse fixture with and without `runtime` field; assert detected agents carry the right `RuntimeKind`.
  - **Depends on Task 1.**

<!-- Commit checkpoint: tasks 4-6, 6b → "feat(surge-acp): map SandboxMode to native flags for Claude/Codex/Gemini and wire registry runtime mapping" -->

### Phase 3: Elevation roundtrip

- [x] **Task 7: Wire ACP `RequestPermissionRequest` into the surge event log as `SandboxElevationRequested`.**
  - Translation point lives in `crates/surge-acp/src/bridge/session.rs` and `crates/surge-acp/src/bridge/client.rs` (where `request_permission` is currently referenced — verify with `grep request_permission crates/surge-acp/src/bridge/`). Top-level bridge lifecycle in `bridge/acp_bridge.rs` + `bridge/worker.rs` does not need direct changes.
  - Add a `BridgeEvent::PermissionRequest { session_id, request: RequestPermissionRequest, reply: PermissionReplyHandle }` variant to `crates/surge-acp/src/bridge/event.rs` (or extend the existing event shape there — inspect first). `PermissionReplyHandle` wraps a `tokio::sync::oneshot::Sender<RequestPermissionResponse>` plus the original `request_id` for correlation.
  - In `crates/surge-orchestrator/src/engine/stage/agent.rs` (already subscribes to bridge events), observe `PermissionRequest` and append `EventPayload::SandboxElevationRequested { node, capability }` via the engine's existing event-append path. `capability` format: `"<kind>:<details>"` (e.g., `"fs-write:./src/foo.rs"`, `"network:api.example.com"`, `"shell:cargo run"`), derived from `RequestPermissionRequest.tool_call.tool_name` and arguments.
  - Add `crates/surge-orchestrator/src/engine/elevation.rs` (new) holding a `PendingElevation` registry keyed by `(SessionId, request_id)` with the reply oneshot.
  - **Files:** `crates/surge-acp/src/bridge/session.rs`, `crates/surge-acp/src/bridge/client.rs`, `crates/surge-acp/src/bridge/event.rs`, `crates/surge-orchestrator/src/engine/stage/agent.rs`, `crates/surge-orchestrator/src/engine/elevation.rs`.
  - **Logging:** `tracing::info!(session = %sid, capability = %cap, "elevation requested by agent")`. `tracing::debug!(target: "surge_orch.elevation")` per registry insert / fulfill / drop. `tracing::warn!(target: "surge_orch.elevation", pending_count = %n, "elevation pending registry growing")` when the registry exceeds the size threshold (default 32).
  - **Tests:** integration test against the existing mock ACP agent (`crates/surge-acp/src/bin/mock_acp_agent.rs`) — emit `RequestPermissionRequest`, assert `SandboxElevationRequested` appears in the event log at a deterministic seq.
  - **Depends on Task 4.**

- [x] **Task 8: Route elevation decisions through `surge-notify` and back to ACP.**
  - `crates/surge-orchestrator/src/engine/elevation.rs` dispatches a notification via the existing `NotificationChannel` trait, one per declared `elevation_channels` in `ApprovalConfig` (lives in `crates/surge-core/src/approvals.rs:5-22`; already has `elevation: bool` defaulted to `true` and `elevation_channels: Vec<ApprovalChannel>` fields). REUSE existing card/template surface — do **not** add a new notify trait.
  - ADD to `ApprovalConfig`: `pub elevation_timeout: Option<humantime_serde::Serde<Duration>>` with default 24h (use existing `humantime-serde` workspace dep).
  - On approval decision: append `EventPayload::SandboxElevationDecided { node, decision: ElevationDecision::*, remember }` (existing payload shape — see run_event.rs:266-270) and fulfil the `PendingElevation` oneshot with `RequestPermissionResponse { outcome: Selected { option_id } | Cancelled, meta: None }`.
  - Handle `AllowAndRemember` by writing a session-scoped allowlist entry that the resolver consults before the next request (in-memory only; survives session lifetime, not daemon restart — Out of Scope crash-recovery beyond this milestone).
  - Timeout policy: when timer fires, append `SandboxElevationDecided { decision: Deny, remember: false }` AND a separate NEW `EventPayload::SandboxElevationTimedOut { node, capability }` event variant (cleaner than extending the existing `SandboxElevationDecided` payload shape with a `reason` field). Schema migration handled in Task 8b.
  - **Files:** `crates/surge-orchestrator/src/engine/elevation.rs`, `crates/surge-core/src/approvals.rs`, `crates/surge-core/src/run_event.rs` (new `SandboxElevationTimedOut` variant), `crates/surge-acp/src/bridge/session.rs`, `crates/surge-notify/` (consumer-side wiring only).
  - **Logging:** `tracing::info!(decision = ?d, channel = %ch, "elevation decided")`. `tracing::warn!(node = ?n, "elevation timed out, denying by default")` on timeout. `tracing::error!(session = ?sid, "no pending elevation for session/request; orphan decision")` on unreachable session.
  - **Tests:** integration test driving the full loop (mock ACP request → mock notify channel → injected decision → assert agent receives `RequestPermissionResponse` with the expected outcome). Timeout test using `tokio::time::pause()` + `advance()`.
  - **Depends on Task 7.**

- [x] **Task 8b: Schema migration v1 → v2 for new event variants.**
  - Tasks 8 and 12 introduce two new `EventPayload` variants (`SandboxElevationTimedOut` and `RuntimeVersionWarning`). Bump `MAX_SUPPORTED_VERSION` to 2 in `crates/surge-core/src/migrations/mod.rs` and add a `MigrationV1ToV2` impl of the `Migration` trait that defers to the v1 identity decoder (additive change — old persisted bytes never contained the new variants, so they parse cleanly).
  - Update `MigrationChain::new()` to register `MigrationV1ToV2`. Update `VersionedEventPayload::new(payload)` to write `schema_version: 2` by default.
  - **Files:** `crates/surge-core/src/migrations/mod.rs`, `crates/surge-core/src/run_event.rs`.
  - **Logging:** none (leaf crate).
  - **Tests:** round-trip a v1-encoded payload through `MigrationV1ToV2` → identical `EventPayload`. Round-trip a payload containing each new variant through `schema_version=2` → identical `EventPayload`. Reject `schema_version > MAX_SUPPORTED_VERSION` with a clear error.
  - **Depends on Tasks 8, 12.**

- [x] **Task 9: Negative tests — blocked elevation surfaces as `StageFailed`, unsupported combo refuses run start.**
  - Add `crates/surge-orchestrator/tests/elevation_blocked.rs`: drive an agent stage that requests elevation, deny it, assert the outcome is `StageFailed` with `reason` referencing the denied capability.
  - Add `crates/surge-orchestrator/tests/sandbox_unsupported_combo.rs`: load a `flow.toml` with `runtime = "gemini"` + `mode = "read-only"` (a row marked `verified = false` with empty flags), assert `engine run` refuses with the typed sandbox-resolve error and the daemon never appends `RunStarted`.
  - **Files:** `crates/surge-orchestrator/tests/elevation_blocked.rs`, `crates/surge-orchestrator/tests/sandbox_unsupported_combo.rs`.
  - **Logging:** assert `error!` is emitted (use `tracing-subscriber` test layer with a capturing writer; or assert exit code + error message).
  - **Depends on Tasks 6, 8, 8b.**

<!-- Commit checkpoint: tasks 7-8, 8b → "feat: wire ACP elevation roundtrip with schema v2"; task 9 → "test: negative cases for elevation and sandbox refusal" -->

### Phase 4: `surge doctor` and version policy

- [x] **Task 10: Add `surge doctor` top-level command.**
  - New `crates/surge-cli/src/commands/doctor.rs` with `clap`-derived subcommands: `surge doctor` (full report), `surge doctor agent <name>` (smoke session), `surge doctor matrix` (just print the matrix, machine-readable `--format=json|toml|text`).
  - Full report includes: detected agents on PATH (reuse `Registry::detect_installed_with_paths()`), version of each detected binary (call `<bin> --version` with short timeout, parse semver out of the first line, structured error on parse failure), matrix row for each `(runtime × mode)` with a column for verified/unverified/unsupported.
  - Add `Doctor` variant to the top-level `Commands` enum in `crates/surge-cli/src/commands/mod.rs`.
  - **Files:** `crates/surge-cli/src/commands/doctor.rs`, `crates/surge-cli/src/commands/mod.rs`, `crates/surge-cli/src/main.rs` (dispatch).
  - **Logging:** `tracing::info!` per top-level step. `tracing::debug!` for each binary probe with stdout excerpt. `tracing::warn!` when a probe times out (≥1s) or returns a non-semver string. Anyhow at the binary boundary only.
  - **Tests:** `crates/surge-cli/tests/doctor.rs` — smoke test running `surge doctor matrix --format=json` and asserting key fields; mock the PATH probe via a tempdir of shell stubs.
  - **Depends on Task 3, 6.**

- [x] **Task 11: `surge doctor agent <name>` smoke session via mock-or-real ACP agent.**

  > **Implementation note:** Mock-path smoke (matrix dry-run + agent
  > registry lookup) is wired and tested. Real ACP smoke session (gated
  > by `SURGE_DOCTOR_REAL=1`) is stubbed with a clear warn message — the
  > full integration requires threading the surge-acp `Registry` into
  > engine session-open paths, which is invasive enough to belong with
  > the version-policy enforcement deferral noted in Task 12. The CLI
  > surface and operator UX are complete; the real-session probe lands
  > together with the agent-stage registry plumbing in follow-up work.
  - Open a real ACP session against the named registered agent, send a minimal canned prompt ("respond OK"), wait for `SessionEstablished` + `AgentMessageChunk` + `SessionEnded`. Report success with token usage; report failure with stage (init / new_session / prompt / close) and excerpt.
  - Reuse `surge-acp::testing::MockAgent` for the unit test of this command. The real smoke is end-to-end and gated by env var `SURGE_DOCTOR_REAL=1` (CI-friendly; default off).
  - Include a "matrix dry-run" column: for the agent's runtime, attempt `resolve_launch_flags(runtime, default_workspace_write_config(), &default_matrix())` and report the resolved flags (without launching).
  - **Files:** `crates/surge-cli/src/commands/doctor.rs`, `crates/surge-cli/tests/doctor_agent.rs`.
  - **Logging:** `tracing::info!` per stage transition; `tracing::error!` on failure with stage label.
  - **Tests:** mock-agent path (always on in CI); real-agent path behind `SURGE_DOCTOR_REAL=1`.
  - **Depends on Task 10.**

- [x] **Task 12: Version policy enforcement during run start (warn-only).**

  > **Implementation note:** Task delivered the `probe_version` utility +
  > `VersionCache` + `evaluate_against_policy` + `RuntimeVersionWarningPayload`
  > infrastructure in `crates/surge-orchestrator/src/engine/version_probe.rs`
  > with full tests. Wiring into the agent stage's session-open path (so the
  > warning event is appended automatically) is deferred to follow-up work
  > alongside the `surge doctor` command (Task 10) — both consumers reuse
  > the same probe utility and require threading the surge-acp registry
  > into `AgentStageParams`, which is invasive and out of scope for this
  > milestone slice.
  - In `crates/surge-orchestrator/src/engine/stage/agent.rs` (where the bridge opens a session for an agent), call `RuntimeVersionPolicy::version_policy(runtime)` and probe the binary version. Cache the probe result in `crates/surge-orchestrator/src/engine/engine.rs` keyed by agent name (one probe per agent registration per daemon lifetime). On `min_version` violation, emit `tracing::warn!` AND append `EventPayload::RuntimeVersionWarning { runtime, found_version: String, min_version: String }` (NEW variant — schema migration is Task 8b). Warn-only — do not refuse the run.
  - Probe: spawn `<binary> --version` via `tokio::process::Command` with 1 s timeout; parse first whitespace-separated token of the first line for semver via `semver::Version::parse` after stripping a leading `v` if present. Make the probe a function `fn probe_version(bin_path: &Path) -> impl Future<Output = Result<semver::Version, ProbeError>>` so tests can replace it via a function pointer or trait.
  - **Files:** `crates/surge-orchestrator/src/engine/stage/agent.rs`, `crates/surge-orchestrator/src/engine/engine.rs`, `crates/surge-core/src/run_event.rs` (new `RuntimeVersionWarning` variant; schema migration in Task 8b).
  - **Logging:** `tracing::warn!(runtime = ?rt, found = %f, min = %m, "runtime version below declared minimum; proceeding")`. `tracing::debug!(target: "surge_orch.version")` on cache hit / probe success / parse fallback. `tracing::error!` ONLY when the probe spawn itself fails — parse failure is `warn` (run proceeds).
  - **Tests:** unit test for `probe_version` with mocked `Command` (use `tempfile` shell stub or trait-based injection); covers below-min / at-min / above-min / unparseable / spawn-error.
  - **Depends on Task 3.**

<!-- Commit checkpoint: tasks 10-12 → "feat(surge-cli): add surge doctor with sandbox matrix and version checks" -->

### Phase 5: Tests, doctor surface polish, docs

- [ ] **Task 13: End-to-end integration test — full elevation roundtrip via real flow.toml.**
  - Add `crates/surge-orchestrator/tests/elevation_e2e.rs`. Load `examples/flow_elevation_demo.toml` (new) with one `Agent` node that runs the existing `mock_acp_agent` (`crates/surge-acp/src/bin/mock_acp_agent.rs`) configured to issue a `RequestPermissionRequest` for a write capability mid-turn. Inject an auto-approve fake `NotificationChannel`.
  - Assert exact ordered seq of `EventPayload` variants: `RunStarted → StageEntered → SandboxElevationRequested → SandboxElevationDecided → OutcomeReported → EdgeTraversed → RunCompleted`.
  - NOTE: `ApprovalRequested` / `ApprovalDecided` are HumanGate-stage events, NOT sandbox elevation. The elevation flow uses `SandboxElevationRequested` / `SandboxElevationDecided` directly; notification dispatch is out-of-band (no extra event).
  - Verify replay determinism: fold the event log from `seq=0` and assert byte-identical state.
  - **Files:** `crates/surge-orchestrator/tests/elevation_e2e.rs`, `examples/flow_elevation_demo.toml`.
  - **Logging:** test uses `tracing-subscriber` test layer; asserts `info!` lines present in expected order via a captured-output helper.
  - **Depends on Tasks 8, 9.**

- [x] **Task 14: Property test — sandbox resolver is total for every declared `(runtime × mode)` row.**
  - In `crates/surge-core/src/sandbox_matrix.rs`, add a `proptest` strategy producing every `(RuntimeKind, SandboxMode)` pair. For every pair, `default_matrix().lookup(runtime, mode)` either returns a row or `None`; `unsupported(runtime, mode)` returns the negation. Property: no pair panics, no pair returns a row with empty `flags` AND `verified = true`.
  - **Files:** `crates/surge-core/src/sandbox_matrix.rs` (tests block).
  - **Depends on Task 1.**

- [x] **Task 15: Audit-logging assertion test.**
  - Add `crates/surge-orchestrator/tests/elevation_audit.rs`: assert every `SandboxElevationRequested` / `SandboxElevationDecided` event carries `node`, `capability`, decision/reason, and timestamp from the event envelope; assert no PII (raw prompt content) leaks into the payload. Use `insta` snapshot for the payload shape.
  - This formalizes the roadmap deliverable "every elevation request + decision recorded with command summary" without adding a separate audit log file.
  - **Files:** `crates/surge-orchestrator/tests/elevation_audit.rs`.
  - **Depends on Task 8.**

<!-- Commit checkpoint: tasks 13-15 → "test(surge): integration tests for sandbox refusal, elevation, doctor" -->

- [ ] **Task 16: Documentation — sandbox matrix table and elevation runbook.**
  - Add `docs/sandbox-matrix.md`: full table generated from `default_matrix()` (consider a small `xtask` to render, or hand-write and add a CI lint that compares the table to the bundled matrix — pick the smaller diff). Columns: runtime, mode, verified, flags, min_version, note.
  - Add `docs/elevation-runbook.md`: explains the lifecycle (`RequestPermissionRequest` → `SandboxElevationRequested` → notify card → `ApprovalDecided` → `SandboxElevationDecided` → `RequestPermissionResponse`), the timeout/deny default, the `AllowAndRemember` session-scoped allowlist, and the audit-trail story.
  - Update `docs/ARCHITECTURE.md` and `CLAUDE.md` to mention the new `surge doctor` command and link the two new docs.
  - Update `.ai-factory/DESCRIPTION.md` "Sandbox delegation" bullet to reference `docs/sandbox-matrix.md`.
  - Run `/aif-docs` checkpoint before marking complete (per Docs: yes setting).
  - **Files:** `docs/sandbox-matrix.md`, `docs/elevation-runbook.md`, `docs/ARCHITECTURE.md`, `CLAUDE.md`, `.ai-factory/DESCRIPTION.md`.
  - **Depends on Tasks 6, 8, 11.**

<!-- Commit checkpoint: task 16 → "docs: sandbox delegation matrix and elevation runbook" -->

## Out of Scope (deferred, captured here for traceability)

- `--trace-acp` flag (Active Summary risk register). Reasoning: cuts across every ACP call site; deserves its own milestone slice.
- Pinning `agent-client-protocol` SDK to a specific rev with a `cargo-lock` policy file. Reasoning: orthogonal infra concern; not blocked by anything here.
- JetBrains Junie verification, Copilot CLI verification post-GA, OpenCode meta-backend recommendation in `surge init`. Reasoning: matrix infra makes these one PR each after upstream stabilizes; deciding the matrix shape now is the prerequisite.
- Profile trust / signature story.

## Acceptance Criteria

1. `surge engine run` against any `flow.toml` requesting an unsupported `(runtime, mode)` combo refuses with a typed error before opening a session, no silent downgrade.
2. The ACP elevation roundtrip is observable in the event log (`SandboxElevationRequested` → `SandboxElevationDecided`) for every `RequestPermissionRequest` the agent issues.
3. `surge doctor` prints a complete matrix in text/json/toml; `surge doctor agent <name>` runs a mock-agent smoke session and reports stage-by-stage success.
4. Cargo build clean across the workspace; `cargo clippy --workspace -- -D warnings` clean; `cargo test --workspace` green; replay-determinism property test passes.
5. `docs/sandbox-matrix.md` and `docs/elevation-runbook.md` exist and match the bundled matrix; ROADMAP.md "Sandbox delegation matrix" milestone moved to Completed.
