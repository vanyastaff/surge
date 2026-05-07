[← Architecture](ARCHITECTURE.md) · [Back to README](README.md) · [Archetypes →](archetypes.md)

# Hooks

Hooks are short, deterministic shell commands that run alongside agent
stages. They give a flow author a place to enforce repository-specific
guarantees — formatting, security checks, retry logic — without changing
the engine.

> **Status:** ships in Surge `v0.1` Graph engine GA. Profile-level
> `extends` inheritance is intentionally **deferred** to the
> `Profile registry & bundled roles` milestone; the executor today
> operates on a single resolved profile (or `AgentConfig.hooks`
> directly).

## Lifecycle — when each trigger fires

The four hook triggers cover every observable transition inside an
`Agent` stage:

| Trigger          | Fires                                                                     | Engine action on rejection                                                                                                  |
|------------------|---------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------|
| `pre_tool_use`   | Before the engine dispatches a non-injected ACP tool call.                | Engine sends a synthetic `ToolResultPayload::Error` reply to the agent and **skips** the dispatcher entirely.                |
| `post_tool_use`  | After the dispatcher replies — i.e. after the agent receives the result.  | Cannot un-run the call. Engine logs a warning and emits `HookExecuted` for audit.                                            |
| `on_outcome`     | Before `OutcomeReported` is appended to the event log.                    | Engine appends `OutcomeRejectedByHook`, increments the per-session attempt counter, and lets the agent pick another outcome. |
| `on_error`       | When a stage fails (tool spawn failure, ACP transport error, etc.).       | Engine may **suppress** the failure into a declared outcome via a JSON stdout directive (see "Suppression" below).           |

Every hook invocation appends one `HookExecuted { hook_id, exit_status, on_failure }`
event to the run log. These are deterministic pass-throughs at fold time
— `RunMemory` only mutates on `OutcomeReported`, never on `HookExecuted`
or `OutcomeRejectedByHook`.

## Matching — `MatcherSpec`

Each hook is gated by a structured matcher. An empty matcher matches
every event of the configured trigger; setting a field is an additional
`AND` constraint:

```toml
[[hooks]]
id = "fmt-check"
trigger = "post_tool_use"
command = "cargo fmt --check"
on_failure = "warn"
timeout_seconds = 30

[hooks.matcher]
tool = "edit_file"          # exact tool-name match
file_glob = "**/*.rs"       # standard `glob` pattern, matched via Pattern::matches_path
```

| Field                | Type           | Match rule                                                            |
|----------------------|----------------|------------------------------------------------------------------------|
| `tool`               | `String`       | Exact equality on the tool name carried by `BridgeEvent::ToolCall`.    |
| `outcome`            | `OutcomeKey`   | Exact equality on the candidate outcome (used by `on_outcome`).        |
| `node`               | `NodeKey`      | Exact equality on the node currently executing.                        |
| `tool_arg_contains`  | `String`       | Substring match against `args_redacted_json`.                          |
| `file_glob`          | `String`       | `glob::Pattern::matches_path` against the contextual file path.        |

If `file_glob` is set but the engine has no path to match against
(e.g. a tool that doesn't operate on a file), the matcher returns
`false` — the hook simply does not run.

## Failure modes — what `on_failure` does

The hook process exits with a status. The mapping table:

| Exit | `on_failure = "reject"`                                                | `on_failure = "warn"`                  | `on_failure = "ignore"` |
|------|-------------------------------------------------------------------------|----------------------------------------|--------------------------|
| `0`  | Continue chain.                                                         | Continue chain.                        | Continue chain.          |
| ≠ 0  | **Short-circuit** — chain returns `HookOutcome::Reject { reason }`.     | Log a `WARN`, continue chain.          | Silent continue.         |
| `124`| Same as `≠ 0`. The runtime sets exit `124` when the per-hook timeout fires. |                                  |                          |

A `Reject` from `pre_tool_use` cancels the dispatch. A `Reject` from
`on_outcome` retries until `AgentLimits::max_retries` is exhausted, at
which point the engine writes `StageFailed { reason, retry_available: false }`.
A `Reject` from `on_error` is treated as `Proceed` — once a stage has
already failed, you cannot reject it harder.

## Suppression — turning errors into outcomes

`on_error` hooks can **convert** a stage failure into a successful
`OutcomeReported` by emitting a single-line JSON directive on stdout:

```json
{"action":"suppress","outcome":"retry_later"}
```

The supplied outcome must be declared on the failing node. If it is
not declared, the engine logs a `WARN [hooks][on_error]
suppression-with-undeclared-outcome` and lets the original `StageFailed`
event fire as if the hook had returned `Proceed`.

Suppression directives are only honoured for the `on_error` trigger —
`pre_tool_use`, `post_tool_use`, and `on_outcome` ignore them.

## Profile authoring example

```toml
schema_version = 1

[role]
id = "rust-implementer"
version = "1.0.0"
display_name = "Rust Implementer"
category = "agents"
description = "Writes idiomatic Rust"
when_to_use = "Rust crates"

[runtime]
recommended_model = "claude-opus-4-7"

[[outcomes]]
id = "done"
description = "Implementation complete"
edge_kind_hint = "forward"

[prompt]
system = "You are a Rust expert."

# Hook 1 — block writes to vendored sources.
[[hooks.entries]]
id = "deny-vendor-writes"
trigger = "pre_tool_use"
command = "exit 1"
on_failure = "reject"

[hooks.entries.matcher]
tool = "edit_file"
file_glob = "vendor/**"

# Hook 2 — auto-format any edited Rust file.
[[hooks.entries]]
id = "fmt-after-write"
trigger = "post_tool_use"
command = "cargo fmt"
on_failure = "warn"
timeout_seconds = 30

[hooks.entries.matcher]
tool = "edit_file"
file_glob = "**/*.rs"

# Hook 3 — gate the `done` outcome on tests passing.
[[hooks.entries]]
id = "tests-must-pass"
trigger = "on_outcome"
command = "cargo test --quiet"
on_failure = "reject"
timeout_seconds = 600

[hooks.entries.matcher]
outcome = "done"

# Hook 4 — recover transient ACP timeouts into a retry outcome.
[[hooks.entries]]
id = "transient-recover"
trigger = "on_error"
command = "scripts/classify-error.sh"
on_failure = "warn"
```

## Determinism guarantees

`RunState::apply()` treats `HookExecuted` and `OutcomeRejectedByHook`
as no-op pass-throughs. Replaying the event log produces identical
state byte-for-byte regardless of how many hooks ran or rejected — the
audit trail is preserved without leaking into engine memory.

The matcher functions in `surge_core::hooks` are pure (no I/O, no
clock), so a graph that validates against a snapshot in CI will keep
matching the same hooks forever. Hook *commands* themselves are
side-effecting by nature; the engine only guarantees that their
*decisions* are observed and recorded deterministically.

## See also

- [`docs/archetypes.md`](archetypes.md) — example flows that use hooks.
- [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) — engine and ACP bridge.
- [`crates/surge-orchestrator/src/engine/hooks/mod.rs`](../crates/surge-orchestrator/src/engine/hooks/mod.rs) — `HookExecutor` source.
