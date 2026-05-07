---
status: accepted
deciders: vanyastaff
date: 2026-05-07
supersedes: none
---

# ADR 0001 — Profile registry layout

## Context

The `Profile registry & bundled roles` milestone (next on the roadmap after Graph Engine GA) needs to ship:

- A registry that resolves a profile reference (e.g. `implementer@1.0`) to a fully merged `Profile` value.
- Disk lookup under `~/.surge/profiles/` honoring a `SURGE_HOME` override.
- A bundled fallback covering 17 first-party roles (3 bootstrap + 7 execution + 4 specialized + 2 project + `mock@1.0`).
- Handlebars-rendered system prompts.
- A `surge profile {list,show,validate,new}` CLI surface.

Without an explicit layout decision, this code could plausibly land in any of three crates: `surge-core`, `surge-orchestrator`, or `surge-cli`. The architecture doc (`.ai-factory/ARCHITECTURE.md` § Dependency Rules) constrains the answer: `surge-core` must remain I/O-free; adapters must not see each other; binaries sit at the top of the graph.

## Decision

Profile registry code is split across three crates along the existing layer boundaries:

1. **`surge-core::profile::registry`** — pure inheritance and merge logic.
   - `ResolvedProfile`, `Provenance`, `merge_chain(parent, child) -> ResolvedProfile`.
   - `MAX_EXTENDS_DEPTH = 8`, cycle detection, depth guard.
   - `parse_key_ref(s) -> ProfileKeyRef { name, version }` for `name@version` syntax.
   - No filesystem access. No `tokio`. Property tests (proptest + insta) live alongside.

2. **`surge-core::profile::bundled`** — embedded asset access.
   - `BundledRegistry::all()` returns the 17 bundled profiles from `crates/surge-core/bundled/profiles/*.toml` via `include_str!`.
   - Compile-time inlined; no runtime asset loading; no `rust-embed` dependency.

3. **`surge-orchestrator::profile_loader`** — disk I/O and 3-way resolution.
   - `surge_home()` and `profiles_dir()` honoring `SURGE_HOME`, mirroring `surge-persistence::store::default_path`.
   - `DiskProfileSet::scan` walking `*.toml` flat under `profiles_dir()`; warn-and-skip on parse failure.
   - `ProfileRegistry::{load, resolve, list}` with the lookup order **versioned (exact `role.version` match on disk) → latest (highest `role.version` on disk for the requested name) → bundled fallback**, tagging each result with `Provenance::{Versioned, Latest, Bundled}`.
   - Version match is **canonical against `Profile.role.version`** (the semver in the TOML body); the filename is only a hint to humans and a duplicate-detection key. `name.toml`, `name-1.0.toml`, and `name-2.0.toml` are all candidates for the latest-disk lane and the highest body-version among them wins.

4. **`surge-cli::commands::profile`** — user-facing CLI.
   - `surge profile list` with provenance column + optional `--format json`.
   - `surge profile show <name> [--version X.Y.Z] [--raw]` rendering merged or raw profile as TOML.
   - `surge profile validate <path>` checking schema + Handlebars syntax + `extends` parent existence.
   - `surge profile new <name> [--base BASE]` scaffolding from a chosen base; refuses to overwrite.

## Cross-cutting choices locked here

- **Asset bundling: `include_str!`**, not `rust-embed`. ~17 small TOML files; compile-time inlining; zero runtime cost.
- **Template engine: Handlebars** in strict mode (no HTML escape). Lives in `surge-orchestrator`. Renders `Profile.prompt.system` only (the actual `PromptTemplate` field is `system: String`).
- **`SURGE_HOME` env var** overrides `~/.surge`; falls back to `dirs::home_dir().join(".surge")`, matching `surge-persistence::store::default_path` behavior.
- **Mock preserved.** The M5 special-case `if profile_str == "mock"` is removed and replaced by a bundled `mock@1.0` profile whose `runtime.agent_id = "mock"` resolves to `AgentKind::Mock` through the agent registry.
- **Profile→AgentKind binding.** `RuntimeCfg` gains a new `agent_id: String` field (default `"claude-code"`) referencing `surge_acp::RegistryEntry::id`. The engine resolves the binary path via the existing agent registry. Without this field there is no derivable `AgentKind` from a profile.
- **Engine wiring.** `EngineConfig` gains `profile_registry: Option<Arc<ProfileRegistry>>`. A new `Engine::new_full(...)` constructor takes the full set of dependencies; legacy constructors delegate. `AgentStageParams` gains a `profile_registry` field mirroring the existing `mcp_registry` pattern.

## Merge semantics (shallow)

Child profile fields override the parent according to the field shape that actually exists in `surge_core::profile::Profile`:

- `runtime` scalars → child wins on non-default.
- `tools` (`default_mcp`, `default_skills`, `default_shell_allowlist`) → three `Vec<String>` fields, each fully replaced by the child when present.
- `outcomes` (`Vec<ProfileOutcome>`) → child fully replaces parent.
- `bindings.expected` (`Vec<ExpectedBinding>`) → merged by `name` field; child overrides matching entries; parent's other entries preserved.
- `hooks.entries` (`Vec<Hook>`) → union dedup by `Hook::id`; child wins on collision (logged at WARN).
- `prompt.system` (`String`) → child wins when non-empty.
- `inspector_ui.fields` (`Vec<InspectorUiField>`) → child fully replaces parent.
- `sandbox` (`SandboxConfig`) → child wins as a whole.

## Alternatives considered

- **Single-crate registry (`surge-orchestrator` only).** Rejected: pure merge / inheritance logic does not require I/O, and centralizing it in `surge-core` keeps the merge code testable without spawning the runtime. The leaf invariant in `.ai-factory/ARCHITECTURE.md` § Dependency Rules makes this the natural home.
- **`rust-embed` for asset bundling.** Rejected: 17 small TOML files do not justify a build-time codegen dependency. `include_str!` is one line per asset and produces identical compile-time inlining.
- **Tera or askama for templating.** Rejected: the bundled prompt set uses simple variable substitution and a few conditionals; Handlebars is already widely understood, and the roadmap milestone explicitly names it.
- **Runtime download from a remote registry.** Deferred — see `roadmap.md` Out-of-Scope and ADR 0002.

## Consequences

- `surge-core` gains two new modules (`profile::registry`, `profile::bundled`) but stays I/O-free.
- `surge-orchestrator` owns the only filesystem-touching profile code and is the only place `SURGE_HOME` is read.
- The legacy M5 `if profile_str == "mock"` fast path is deleted; `mock@1.0` becomes a bundled profile and goes through the same resolution chain as everything else.
- `RuntimeCfg` gains a new required-with-default field. Existing TOML profiles continue to parse because `agent_id` defaults to `"claude-code"` via `#[serde(default = ...)]`.
- The CLI gains four new subcommands; their bodies depend on the registry being loadable, so the engine wiring (Task 30) must precede end-to-end CLI smoke tests.
