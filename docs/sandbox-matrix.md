# Sandbox Delegation Matrix

Surge does not implement its own sandbox enforcement. Instead it **delegates** sandboxing to the agent runtime — passing the right launch flags so the agent's own native sandbox enforces the requested capability tier. The matrix below maps each surge `SandboxMode` to the launch flags surge appends to each supported runtime's command line.

> **Source of truth**: [`crates/surge-core/bundled/sandbox/matrix.toml`](../crates/surge-core/bundled/sandbox/matrix.toml).
> Inspect at any time with `surge doctor matrix --format json|toml|text`.

## Capability tiers

| Mode | Intent |
| --- | --- |
| `read-only` | Read filesystem, no writes, no shell, no network |
| `workspace-write` | Read + write within the worktree; no shell, no network (default) |
| `workspace-network` | `workspace-write` + outbound network egress |
| `full-access` | Unrestricted; the agent runtime owns all enforcement |
| `custom` | Caller-defined via `SandboxConfig` allowlists; validated by `validate_custom()` |

## Verified rows (v0.1)

These rows are tested against the live runtime and are safe to use in production runs.

| Runtime | Mode | Flags | Min. version |
| --- | --- | --- | --- |
| `claude-code` | `read-only` | `--allowedTools "Read,Glob,Grep,LS" --disallowedTools "Write,Edit,Bash,WebFetch,WebSearch"` | `>=2.0.0` |
| `claude-code` | `workspace-write` | `--allowedTools "Read,Write,Edit,Glob,Grep,LS" --disallowedTools "Bash,WebFetch,WebSearch"` | `>=2.0.0` |
| `claude-code` | `workspace-network` | `--allowedTools "Read,Write,Edit,Glob,Grep,LS,WebFetch,WebSearch" --disallowedTools "Bash"` | `>=2.0.0` |
| `claude-code` | `full-access` | `--dangerously-skip-permissions` | `>=2.0.0` |
| `codex` | `read-only` | `--sandbox=read-only` | `>=0.5.0` |
| `codex` | `workspace-write` | `--sandbox=workspace-write` | `>=0.5.0` |
| `codex` | `workspace-network` | `--sandbox=workspace+network` | `>=0.5.0` |
| `codex` | `full-access` | `--sandbox=danger-full-access` | `>=0.5.0` |
| `gemini` | `full-access` | `--yolo` | `>=0.4.0` |

## Declared but unverified

Surge declares awareness of these runtimes but has not yet tested the live launch flags. Production runs against these rows refuse with `SandboxResolveError::UnverifiedRuntime`. Operators can still probe them through `surge doctor agent <name>` (the doctor `ResolveContext` relaxes this check so the matrix can be inspected without launching).

| Runtime | Modes | Reason |
| --- | --- | --- |
| `gemini` | `read-only`, `workspace-write`, `workspace-network` | Gemini's native sandbox is Docker/Podman-only — surge cannot enforce tiered modes against a non-Docker launch. Refuses rather than silently downgrading. |
| `cursor` | all 4 modes | `cursor.com/docs/cli/acp` flag forms not yet confirmed. |
| `copilot` | all 4 modes | GitHub Copilot CLI ACP is in public preview; flag forms pending GA. |
| `opencode` | all 4 modes | OpenCode delegates to a configured provider; surge has not yet mapped the host-side flags. |
| `goose` | all 4 modes | Goose ACP surface stable; sandbox CLI flags not yet enumerated. |

## How surge picks a row

1. Authors declare `sandbox.mode` on an Agent node (or inherit it from the profile) and optionally specify a `runtime` via the agent registry entry.
2. At session-open time, `surge-acp::bridge::sandbox_resolver::resolve_launch_flags(runtime, cfg, matrix, ctx)` looks up the matching row.
3. **`ResolveContext::Run`** (production): unverified rows return `SandboxResolveError::UnverifiedRuntime`. Unsupported combos return `SandboxResolveError::UnsupportedCombo`. `Custom` mode delegates to `validate_custom()`.
4. **`ResolveContext::Doctor`**: declared-unverified rows are allowed through (with empty flags) so `surge doctor` can surface them without launching.

## Adding a new runtime

1. Add a `RuntimeKind` variant in `crates/surge-core/src/runtime.rs` (it is `#[non_exhaustive]`).
2. Add at least one `[[rows]]` entry in `crates/surge-core/bundled/sandbox/matrix.toml`.
   - For verified rows: set `verified = true`, populate `flags`, set `min_version`.
   - For declared-unverified rows: leave `flags = []`, set `verified = false`, write a non-empty `note` linking to the upstream issue or docs page.
3. Add a `runtime = "..."` field to the matching entry in `crates/surge-acp/builtin_registry.json`.
4. (Optional) Declare a `[[policies]]` entry in `crates/surge-core/bundled/sandbox/versions.toml` for warn-only version floor.
5. Run `cargo test -p surge-core --lib sandbox_matrix` — the property test asserts no row is `verified = true` with empty flags, and every unverified row has a non-empty note.

## See also

- [Elevation runbook](elevation-runbook.md) — how operators approve/deny mid-run permission requests.
- [Architecture](ARCHITECTURE.md) — overall sandbox delegation rationale.
- [ADR-0006](adr/0006-acp-only-transport.md) — why surge is ACP-only and the implications for sandbox handoff.
