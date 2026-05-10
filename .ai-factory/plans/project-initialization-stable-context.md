# Implementation Plan: Project Initialization & Stable Context

Branch: feature/project-initialization-stable-context
Created: 2026-05-10

## Settings
- Testing: yes
- Logging: verbose
- Docs: yes

## Roadmap Linkage
Milestone: "Project initialization & stable context"
Rationale: This is the first unchecked milestone in `.ai-factory/ROADMAP.md` after the completed bootstrap/adaptive-flow work.

## Context Snapshot
- Existing `surge init` is currently an inline `Commands::Init` arm in `crates/surge-cli/src/main.rs` that writes a minimal `surge.toml`.
- `surge-acp` already provides registry/discovery primitives via `Registry`, `AgentDiscovery`, and bundled entries for Claude, Codex, and Gemini ACP-compatible agents.
- `surge-core::config::SurgeConfig` already owns agent, pipeline, cleanup, IDE, resilience, analytics, task-source, Telegram, and inbox config parsing/validation.
- `project-context-author@1.0` is already bundled in `crates/surge-core/bundled/profiles/project-context-author-1.0.toml`; no new profile is needed for `surge project describe`.
- `surge bootstrap` now has a CLI command and bootstrap driver patterns that can be reused for profile-driven project-context generation.
- Current bundled profiles use `runtime.agent_id = "claude-code"`, while the built-in ACP registry entries are named `claude-acp`, `codex-acp`, and `gemini`; the implementation must normalize this before relying on production profile execution.
- The engine already seeds `initial_prompt` as an `ArtifactProduced` event and has a content-addressed `ArtifactStore`; `project.md` should follow that path instead of being lazily re-read during stage execution.

## Commit Plan
- **Commit 1** (after tasks 1-4): "feat: add init wizard config model"
- **Commit 2** (after tasks 5-7): "feat: implement init wizard flow"
- **Commit 3** (after tasks 8-11): "feat: implement project context describe"
- **Commit 4** (after tasks 12-15): "feat: bind project context into runs"
- **Commit 5** (after tasks 16-17): "test: cover project onboarding flow"
- **Commit 6** (after tasks 18-19): "docs: document project initialization"

## Tasks

### Phase 1: Init Command Shape And Config Surface
- [x] Task 1: Extract `surge init` into a dedicated command module.
  - Deliverable: Move the inline `Commands::Init` implementation from `crates/surge-cli/src/main.rs` into a new `crates/surge-cli/src/commands/init.rs`, export it from `crates/surge-cli/src/commands/mod.rs`, and keep the top-level clap command behavior compatible.
  - Expected behavior: `surge init` remains available, refuses to overwrite an existing `surge.toml` unless the new idempotent edit flow explicitly allows it, and returns clear human-facing errors from the CLI boundary.
  - Files: `crates/surge-cli/src/main.rs`, `crates/surge-cli/src/commands/mod.rs`, `crates/surge-cli/src/commands/init.rs`.
  - Logging requirements: Use `tracing::debug!` for command entry, discovered cwd, existing-config detection, and generated-section decisions; use `tracing::info!` for successful config creation/update. Keep user-facing status lines as `println!` in CLI only.

- [x] Task 2: Extend `SurgeConfig` with typed initialization defaults.
  - Deliverable: Add typed config fields needed by the wizard: sandbox defaults, managed worktree root/location, approval channels, and initialization/project-context metadata while preserving backwards-compatible defaults.
  - Expected behavior: Existing `surge.toml` files keep deserializing; new fields serialize with sensible names and validate with actionable errors.
  - Files: `crates/surge-core/src/config.rs`, `surge.example.toml`.
  - Logging requirements: No logging inside pure validation helpers unless already present; CLI callers should log values before/after applying defaults at DEBUG while redacting token/env-var contents.
  - Dependency notes: Must respect `surge-core` as the leaf crate; add only pure types/validation here, no agent execution or filesystem scanning beyond existing config load/save helpers.

- [x] Task 3: Implement `surge init --default` for non-interactive onboarding.
  - Deliverable: Add `--default` to the `Init` clap args and generate a complete safe `surge.toml` using `SurgeConfig::save` rather than hand-written TOML strings.
  - Expected behavior: In a fresh repo, `surge init --default` writes a valid config with the best detected ACP agent when available, otherwise a documented `claude-acp`/mock-safe fallback and a warning with next steps.
  - Files: `crates/surge-cli/src/main.rs`, `crates/surge-cli/src/commands/init.rs`, `crates/surge-core/src/config.rs`.
  - Logging requirements: DEBUG log discovery inputs, selected default agent, fallback reason, and final config path; WARN log missing agent/runtime/git prerequisites without exposing secret env values.
  - Dependency notes: Reuse `surge-acp::Registry::builtin()` and `AgentDiscovery`/registry detection instead of adding a second PATH scanner.

- [x] Task 4: Normalize profile runtime IDs against the ACP registry.
  - Deliverable: Decide and implement one compatibility path for profile `runtime.agent_id` values such as `claude-code`, `codex`, and `gemini-cli` so they resolve to built-in registry entries such as `claude-acp`, `codex-acp`, and `gemini` before `project-context-author@1.0` is invoked.
  - Expected behavior: `derive_agent_kind_from_id` no longer fails for bundled profiles; `surge profile show project-context-author` and any production engine path can map the profile runtime to a concrete `AgentKind`.
  - Files: `crates/surge-acp/builtin_registry.json`, `crates/surge-acp/src/registry.rs`, `crates/surge-orchestrator/src/engine/stage/agent.rs`, and bundled profiles under `crates/surge-core/bundled/profiles/` only if the chosen fix is to rename profile IDs.
  - Logging requirements: DEBUG log requested profile runtime ID, normalized registry ID, and derived `AgentKind`; WARN log unknown runtime IDs with the list of known registry aliases.
  - Dependency notes: Keep this in `surge-acp`/`surge-orchestrator`; do not make `surge-core` depend on the registry or any process discovery code.

### Phase 2: Interactive Wizard
- [x] Task 5: Build the interactive `surge init` wizard flow.
  - Deliverable: Implement prompts for detected agent registration, sandbox default, worktree root, approval channels, Telegram env-var setup, and optional MCP servers.
  - Expected behavior: Running `surge init` with no flags walks through choices, offers detected agents first, allows safe defaults with Enter, and writes a validated config atomically.
  - Files: `crates/surge-cli/src/commands/init.rs`, `crates/surge-cli/src/commands/registry.rs` if shared display helpers are extracted.
  - Logging requirements: DEBUG log each wizard step name and chosen enum-like option; do not log raw Telegram tokens, chat IDs, or full env-var values. INFO log final success and sections changed.
  - Dependency notes: Keep prompts synchronous and small; avoid adding a terminal UI dependency unless the existing CLI patterns already require it.

- [x] Task 6: Add idempotent re-run/edit behavior for existing projects.
  - Deliverable: If `surge.toml` exists, `surge init` should show current state and offer to edit individual sections instead of failing immediately, using `toml_edit` in the CLI path when preserving existing comments/order matters.
  - Expected behavior: Existing agent entries, Telegram env-var names, worktree settings, and approvals are preserved unless the user edits that section; `--default` on an existing config should be a no-op or explicit safe refresh, not a destructive rewrite.
  - Files: `crates/surge-cli/src/commands/init.rs`, `crates/surge-core/src/config.rs`.
  - Logging requirements: DEBUG log config load path, section-level diff decisions, and skipped sections; INFO log no-op vs updated states; never log raw TOML when it may contain secrets or local paths.
  - Dependency notes: Use `SurgeConfig::load`/`save` for typed validation and isolate any `toml_edit` mutations in `surge-cli` so the user-facing config remains the only format.

- [x] Task 7: Improve first-run prerequisite diagnostics.
  - Deliverable: Add actionable messages for missing agents, unsupported transport choices, missing git repository, invalid worktree path, and Telegram setup placeholders.
  - Expected behavior: New users get direct next steps instead of raw validation errors; command exits with non-zero status for invalid config and success for warnings that do not block initialization.
  - Files: `crates/surge-cli/src/commands/init.rs`, `crates/surge-cli/src/commands/config.rs`, `crates/surge-acp/src/discovery.rs` only if reusable diagnostics need small additions.
  - Logging requirements: WARN log diagnostic categories and machine-readable reason codes; DEBUG log lower-level probe outcomes such as checked binary names and git discovery failure reason.

### Phase 3: Project Context Generation
- [x] Task 8: Add a `surge project describe` CLI command.
  - Deliverable: Introduce top-level `Project` subcommands with `describe`, including `--output`, `--refresh`, and `--dry-run` flags where useful.
  - Expected behavior: `surge project describe` creates or refreshes `project.md` in the project root by default; `--dry-run` prints whether it would change without writing.
  - Files: `crates/surge-cli/src/main.rs`, `crates/surge-cli/src/commands/mod.rs`, `crates/surge-cli/src/commands/project.rs`.
  - Logging requirements: DEBUG log command args, resolved project root, output path, and dry-run/refresh mode; INFO log generated/no-change outcome.

- [x] Task 9: Implement deterministic project scanning and content hashing.
  - Deliverable: Add a scanner that reads high-signal project files (`AGENTS.md`, `CLAUDE.md`, `README.md`, `Cargo.toml`, `justfile`, `rustfmt.toml`, `clippy.toml`, `surge.toml`) plus git state and produces stable input for Project Context Author.
  - Expected behavior: The scan ignores target/build artifacts, orders paths deterministically, includes enough metadata for stack/build/test inference, and computes a hash used to skip spurious `project.md` rewrites.
  - Files: `crates/surge-orchestrator/src/project_context.rs` or `crates/surge-orchestrator/src/project_context/mod.rs`, `crates/surge-orchestrator/src/lib.rs`.
  - Logging requirements: DEBUG log included/skipped paths, detected stack markers, git branch/dirty-state summary, and content hash; WARN log unreadable optional files and continue.
  - Dependency notes: Keep filesystem/git I/O out of `surge-core`; pure schema structs may live in `surge-core` only if needed across crates.

- [x] Task 10: Add scanner redaction, size budgets, and skip rules.
  - Deliverable: Redact secret-like values from `surge.toml`/env-var examples, cap per-file and total scan bytes, skip known heavy or generated directories, and produce an explicit skipped-files summary for the agent.
  - Expected behavior: `project.md` generation never sends raw tokens, chat IDs, API keys, or huge generated files to the agent; oversized files are summarized by path/size/hash instead of embedded.
  - Files: `crates/surge-orchestrator/src/project_context.rs`, `crates/surge-orchestrator/tests/project_context_test.rs`.
  - Logging requirements: DEBUG log size budget decisions and redaction counts; WARN log only the category/path for potentially sensitive skipped input, never the original value.
  - Dependency notes: Use existing `regex`/hashing dependencies from workspace if needed; avoid introducing a second config format or a broad file-walking dependency unless the standard library is insufficient.

- [x] Task 11: Invoke `project-context-author@1.0` to produce `project.md`.
  - Deliverable: Reuse the profile registry and ACP bridge patterns to run the Project Context Author profile against the deterministic scan context, then write/record `project.md`; build a small one-agent graph or focused orchestration helper with explicit `AgentConfig.bindings` for `worktree_root` and `scan_context`.
  - Expected behavior: The command can use a configured agent when available, surfaces profile/agent failures clearly, and reports `drafted` vs `no_change` outcomes according to the bundled profile contract.
  - Files: `crates/surge-orchestrator/src/project_context.rs`, `crates/surge-cli/src/commands/project.rs`, `crates/surge-core/bundled/profiles/project-context-author-1.0.toml` only if a prompt binding needs a small compatibility adjustment.
  - Logging requirements: DEBUG log selected profile key, selected agent/runtime, generated artifact path, outcome, scan hash, and output hash; ERROR log profile execution failure with run/session IDs where available.
  - Dependency notes: Prefer the existing engine/profile path over a one-off LLM client so project context generation behaves like other agent stages; update the bundled profile prompt only to consume the supplied scan context, not to add a new role.

### Phase 4: Project Context Binding Into Runs
- [x] Task 12: Add run-level project context seeding.
  - Deliverable: Extend `EngineRunConfig` with an optional project-context seed (path/hash/content or equivalent), and have `Engine::start_run` store it through `ArtifactStore` and append an `ArtifactProduced` event under a canonical name such as `project_context`.
  - Expected behavior: The project context used by a run is captured at run start; later edits to `project.md` do not alter replay, resume, or audit meaning.
  - Files: `crates/surge-orchestrator/src/engine/config.rs`, `crates/surge-orchestrator/src/engine/engine.rs`, `crates/surge-core/src/run_state.rs`, `crates/surge-persistence/src/artifacts.rs`.
  - Logging requirements: DEBUG log seed path, byte count, and hash; INFO log only the short run-id/path/hash summary; never log full project context at INFO or above.
  - Dependency notes: Prefer reusing `ArtifactProduced` and `RunMemory.artifacts` over adding a new `EventPayload` variant unless a new event is demonstrably needed.

- [x] Task 13: Make bootstrap, engine, daemon, and inbox starts pass the project context seed.
  - Deliverable: Resolve latest `project.md` before run start in all user-facing paths: `surge bootstrap`, `surge engine run`, daemon-backed start requests, and inbox/tracker `Start` actions.
  - Expected behavior: Runs started after `project.md` generation use it automatically; absence of `project.md` is a DEBUG/WARN-level hint, not a hard error.
  - Files: `crates/surge-cli/src/commands/bootstrap.rs`, `crates/surge-cli/src/commands/engine.rs`, `crates/surge-orchestrator/src/engine/ipc.rs`, `crates/surge-orchestrator/src/engine/daemon_facade.rs`, `crates/surge-daemon/src/server.rs`, `crates/surge-daemon/src/inbox/consumer.rs`.
  - Logging requirements: DEBUG log project context discovery path and byte/hash summary; WARN log unreadable context files with remediation; never log entire project context at INFO or above.
  - Dependency notes: Preserve deterministic replay: context content must be captured as run input/artifact or event payload reference at run start, not lazily re-read mid-run.

- [x] Task 14: Expose project context to agent stages through bindings.
  - Deliverable: Add or document a standard binding pattern for profiles/graphs that need stable project context, using `ArtifactSource::RunArtifact { name = "project_context" }` and/or a dedicated helper so generated bootstrap graphs can bind it consistently.
  - Expected behavior: Description Author, Spec Author, and future generated graphs can consume `project_context` without hardcoding filesystem reads; runs without context continue to work with optional/empty bindings where appropriate.
  - Files: `crates/surge-core/bundled/profiles/description-author-1.0.toml`, `crates/surge-core/bundled/profiles/spec-author-1.0.toml`, `crates/surge-orchestrator/src/engine/stage/bindings.rs`, `crates/surge-orchestrator/src/bootstrap_driver.rs` if graph construction needs a helper.
  - Logging requirements: DEBUG log successful binding resolution and missing optional context; WARN only when a graph requires project context and the seeded artifact is absent.
  - Dependency notes: Keep binding resolution pure over `RunMemory` + artifact files; do not introduce ad hoc reads of repository-root `project.md` inside stage execution.

- [x] Task 15: Align `surge.toml` example and validation with wizard outputs.
  - Deliverable: Update `surge.example.toml` so every wizard-produced field is documented, including local/npx/custom/TCP/MCP-flavored agents, sandbox defaults, worktrees, approvals, Telegram env-var setup, and inbox defaults.
  - Expected behavior: `include_str!("../../../surge.example.toml")` config tests still parse and validate; users can copy the example without hidden required fields.
  - Files: `surge.example.toml`, `crates/surge-core/src/config.rs`.
  - Logging requirements: No runtime logging; validation errors should remain specific enough that CLI callers can log the failing section.

### Phase 5: Tests And Smoke Coverage
- [x] Task 16: Add CLI tests for init flows.
  - Deliverable: Cover `surge init --help`, `surge init --default` in a temp repo, existing-config idempotency, and no-agent fallback behavior.
  - Expected behavior: Tests use `assert_cmd`/`tempfile`, avoid real network calls, and assert the produced `surge.toml` parses with `SurgeConfig`.
  - Files: `crates/surge-cli/tests/cli_init_test.rs`, `crates/surge-cli/tests/cli_config_test.rs` if shared helpers are needed.
  - Logging requirements: Tests should assert user-visible warnings where meaningful; DEBUG logging can be enabled with `RUST_LOG` for diagnosis but should not be required to pass.

- [x] Task 17: Add project context and onboarding smoke tests.
  - Deliverable: Cover deterministic scan hashing, redaction/size budgets, no-change refresh, runtime-agent-id normalization, `surge project describe --dry-run`, Project Context Author invocation with a mock bridge, and the roadmap smoke path: `surge init --default` + `surge project describe` + `surge engine run examples/flow_minimal_agent.toml`.
  - Expected behavior: Unit/integration tests avoid requiring a real ACP agent by default; any real-agent smoke remains ignored or feature-gated like existing real ACP tests.
  - Files: `crates/surge-orchestrator/tests/project_context_test.rs`, `crates/surge-orchestrator/tests/engine_project_context_seed_test.rs`, `crates/surge-cli/tests/cli_project_describe_test.rs`, `crates/surge-cli/tests/examples_smoke.rs`, `crates/surge-orchestrator/tests/profile_registry_e2e.rs`.
  - Logging requirements: DEBUG logs should make hash mismatches and skipped rewrites easy to diagnose; WARN logs for missing optional files should not fail tests.

### Phase 6: Documentation And User Guidance
- [x] Task 18: Update onboarding documentation.
  - Deliverable: Revise `docs/getting-started.md`, `docs/cli.md`, and `docs/workflow.md` so `surge init`, `surge init --default`, and `surge project describe` are the canonical project setup path.
  - Expected behavior: Docs distinguish generated `project.md` from `.ai-factory`/agent context files, explain idempotent refresh, and show a minimal fresh-repo smoke path.
  - Files: `docs/getting-started.md`, `docs/cli.md`, `docs/workflow.md`, `docs/README.md` if a new page is added.
  - Logging requirements: Documentation examples should mention `RUST_LOG=surge=debug` for troubleshooting but not require verbose logs for normal use.

- [x] Task 19: Update top-level agent/developer context after layout changes.
  - Deliverable: Update `AGENTS.md`, `CLAUDE.md`, and `.ai-factory/DESCRIPTION.md` only where the command surface or structural map changed.
  - Expected behavior: New `crates/surge-cli/src/commands/init.rs`, `project.rs`, and any project-context module are represented accurately; no detailed implementation prose is duplicated from docs.
  - Files: `AGENTS.md`, `CLAUDE.md`, `.ai-factory/DESCRIPTION.md`.
  - Logging requirements: No runtime logging; keep docs factual and short.

## Verification
- Run `cargo fmt --all`.
- Run `cargo test -p surge-core config`.
- Run `cargo test -p surge-cli --test cli_init_test`.
- Run `cargo test -p surge-cli --test cli_project_describe_test`.
- Run `cargo test -p surge-cli --test cli_bootstrap_help`.
- Run `cargo test -p surge-cli --test examples_smoke`.
- Run `cargo test -p surge-orchestrator --test project_context_test`.
- Run `cargo test -p surge-orchestrator --test engine_project_context_seed_test`.
- Run `cargo test -p surge-orchestrator --test profile_registry_e2e`.
- Run `cargo test -p surge-orchestrator --test bootstrap_driver_e2e`.
- Run `cargo clippy --workspace --all-targets -- -D warnings`.

## Open Questions
- Should `project.md` live strictly at the repository root, or should `surge.toml` allow overriding its path before v0.1?
- Should `surge init --default` register `mock` when no real ACP agent is found, or prefer a commented/documented `claude-acp` entry that points users toward installing an actual runtime?
- Should Telegram setup in this milestone only write env-var names, or also create the registry SQLite binding token flow now?
