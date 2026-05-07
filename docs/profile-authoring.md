# Profile Authoring Guide

Profiles are the reusable configuration that an `Agent` node in a `flow.toml` resolves against. A profile carries a system prompt (Handlebars template), a launch agent reference, sandbox intent, allowed tools, declared outcomes, hooks, and approval policy. The same profile schema covers the bundled first-party set and any local profile a user drops into `${SURGE_HOME}/profiles/`.

This guide covers the schema, inheritance, prompt templating, the outcome contract, sandbox / approvals / hooks, versioning rules, and the `surge profile` workflow. It supersedes ad-hoc notes in `docs/ARCHITECTURE.md` § 6 — that section now points here.

## Schema (top-level)

A profile is a TOML file with this shape (every section except `[role]`, `[runtime]`, `[[outcomes]]`, and `[prompt]` is optional):

```toml
schema_version = 1

[role]
id            = "implementer"
version       = "1.0.0"           # full semver in the body — filename is just a hint
display_name  = "Implementer"
icon          = "wrench"          # optional
category      = "agents"          # one of: agents | gates | flow | io | _bootstrap
description   = "Writes the code described by a spec."
when_to_use   = "Default execution profile for any normal feature work."
extends       = "generic@1.0"     # optional — see Inheritance below

[runtime]
recommended_model     = "claude-opus-4-7"
default_temperature   = 0.2
default_max_tokens    = 200000
agent_id              = "claude-code"     # surge_acp::Registry id
load_rules_lazily     = false             # optional

[sandbox]
mode                  = "workspace-write" # read-only | workspace-write | workspace-network | full-access | custom
writable_roots        = []
network_allowlist     = []
shell_allowlist       = []
protected_paths       = []

[tools]
default_mcp           = []         # mcp server names exposed by default
default_skills        = []         # skill names available to the agent
default_shell_allowlist = []       # shell commands allowed without further approval

[approvals]
policy                = "on-request"   # untrusted | on-request | never
sandbox_approval      = false
mcp_elicitations      = false
request_permissions   = false
skill_approval        = false
elevation             = true
elevation_channels    = []        # [[approvals.elevation_channels]] entries; type-tagged

[[outcomes]]
id                    = "implemented"
description           = "All spec subtasks implemented and the project compiles."
edge_kind_hint        = "forward"  # forward | backtrack | escalate
required_artifacts    = []         # optional list of artifact names

[[bindings.expected]]
name                  = "spec"
source                = { source = "node_output", from_role = "spec-author" }
optional              = false

[hooks]
# entries = [...]                 # see crates/surge-core/src/hooks.rs for shape

[prompt]
system                = """
You are the Implementer. Implement the work described by `{{spec}}`.
"""

[inspector_ui]
fields                = []         # optional UI hints for the desktop shell
```

### `[runtime].agent_id`

Identifies the agent runtime via `surge_acp::Registry`. The default is `"claude-code"`. Other valid ids: `"codex"`, `"gemini-cli"`, `"mock"` (test-only). The engine derives `AgentKind` by looking the id up in the agent registry, taking the registry entry's `command` as the binary path.

### `[[outcomes]]`

A profile must declare at least one outcome. Outcome ids are referenced from `flow.toml` edges; `edge_kind_hint` tells the Flow Generator which `EdgeKind` to suggest.

### `[[bindings.expected]]`

Each binding declares a template variable the prompt expects, plus where its value comes from. The `source` field is an internally-tagged enum, so authors use the inline-table form:

- `source = { source = "any" }` — accept any binding the engine can satisfy.
- `source = { source = "run_artifact" }` — find by name in `RunMemory::artifacts`.
- `source = { source = "node_output", from_role = "spec-author" }` — read the latest artifact produced by a node running the named profile.

## Inheritance

Set `extends = "<base>@<MAJOR>[.<MINOR>[.<PATCH>]]"` to inherit from another profile. The orchestrator walks the chain (max depth 8, cycles rejected) and applies a shallow merge:

| Field | Merge behaviour |
|---|---|
| `runtime.recommended_model` | child wins on non-empty |
| `runtime.default_temperature` / `default_max_tokens` | child wins when distinct from the schema default |
| `runtime.agent_id` | child wins when distinct from the default `"claude-code"` |
| `runtime.load_rules_lazily` | child `Some` wins over parent |
| `tools.default_mcp` / `default_skills` / `default_shell_allowlist` | each list: child wins when non-empty, else parent |
| `outcomes` | child fully replaces parent when non-empty |
| `bindings.expected` | merged by `name`; child overrides matching entries; parent's other entries preserved |
| `hooks.entries` | union dedup by `Hook::id`; child wins on collision (logged at WARN) |
| `prompt.system` | child wins when non-empty |
| `inspector_ui.fields` | child fully replaces parent when non-empty |
| `sandbox`, `approvals` | child wins as a whole when non-default |
| `role` | child wins (the leaf is always its own role) |

The bundled `bug-fix-implementer` is a worked example: it sets `extends = "implementer@1.0"`, overrides `default_temperature`, replaces the outcomes list (`fixed` / `cannot_reproduce` / `blocked`), and rewrites `prompt.system`. Everything else (sandbox, tools, hooks) is inherited.

## Handlebars prompt templates

`prompt.system` is rendered through Handlebars (crate `handlebars` 6) with HTML escaping disabled. Two modes:

- **Strict mode** runs at `ProfileRegistry::load` time so a typo (`{{ unmatched`) fails the registry load with a named error rather than blowing up at agent-launch time.
- **Lenient mode** runs at agent stage execution time so missing optional bindings render as empty strings rather than aborting the run.

Variable references use the standard Handlebars `{{name}}` syntax, mapped from the resolved bindings (`name = "spec"` in `[[bindings.expected]]` exposes `{{spec}}` in the prompt). Conditionals (`{{#if has_adr}}…{{/if}}`) and the standard built-in helpers are available; user-defined helpers are not registered.

## Outcome contract

Each `Agent` stage must report exactly one declared outcome via the injected `report_stage_outcome` tool. The closed enum the agent sees is built from the profile's `[[outcomes]]` list; the engine validates the report against this list before persisting `OutcomeReported`. A `required_artifacts` array on an outcome is documentation for the agent — the engine does not enforce artifact production, but the Verifier profile can.

## Sandbox / approvals / hooks

- **Sandbox.** `mode` is the requested intent; the agent runtime maps it to its native flags. `read-only` blocks workspace writes; `workspace-write` is the default; `workspace-network` adds outbound network; `full-access` removes all guards. `custom` reserves a per-runtime override.
- **Approvals.** `policy = "untrusted"` requires approval for every tool call; `"on-request"` follows the agent's elevation requests; `"never"` disables prompts entirely. `elevation_channels` declares which notification surfaces (Telegram, desktop, email, webhook) receive elevation cards.
- **Hooks.** `[[hooks.entries]]` declares a `pre_tool_use` / `post_tool_use` / `on_outcome` / `on_error` hook with a structured `matcher`, a shell `command`, an `on_failure` mode (`warn` / `reject`), an optional `timeout_seconds`, and an `inherit` flag.

## Versioning

Profiles use full semver in the TOML body (`role.version = "1.0.0"`). The lookup table matches against this, **not** the filename — `implementer-1.0.toml` is just a hint for humans and a duplicate-detection key. References in `flow.toml` and `extends` accept partial forms (`implementer`, `implementer@1`, `implementer@1.0`, `implementer@1.0.0`); partial versions zero-fill the missing positions.

Version conflicts on disk: when two files produce the same `(role.id, role.version)`, the first one the directory walker sees wins; the duplicate is logged at WARN.

## CLI workflow

```sh
# List every visible profile (disk + bundled)
surge profile list
surge profile list --format json

# Render the merged profile after extends resolution
surge profile show implementer
surge profile show implementer --version 1.0.0
surge profile show implementer --raw          # un-merged, original file shape

# Validate a candidate file before adding it to the registry
surge profile validate ./my-profile.toml

# Scaffold a new profile under ${SURGE_HOME}/profiles/
surge profile new my-impl
surge profile new my-impl --base implementer@1.0
```

`surge profile validate` checks: TOML schema, Handlebars syntax of `prompt.system`, and (best-effort) that any `extends` parent exists in the current registry. `surge profile new` refuses to overwrite an existing file — delete the old one explicitly if you mean it.

## Trust and signature

There is no signature verification in v0.1. The registry resolves profiles only from the bundled set (compiled into the binary) and the local `${SURGE_HOME}/profiles/` directory; there is no remote fetch and no publisher allowlist. See [ADR 0002](adr/0002-profile-trust-deferred.md) for the rationale and the conditions under which this should be revisited.

## Where the code lives

| Concern | Crate | Module |
|---|---|---|
| Profile schema | `surge-core` | `profile` |
| Inheritance + merge | `surge-core` | `profile::registry` |
| Bundled assets | `surge-core` | `profile::bundled` |
| `name@version` parser | `surge-core` | `profile::keyref` |
| Disk loader | `surge-orchestrator` | `profile_loader::disk` |
| `${SURGE_HOME}` resolution | `surge-orchestrator` | `profile_loader::paths` |
| 3-way `ProfileRegistry` | `surge-orchestrator` | `profile_loader::registry` |
| Handlebars renderer | `surge-orchestrator` | `prompt` |
| `surge profile` CLI | `surge-cli` | `commands::profile` |

See also: [ADR 0001 — Profile registry layout](adr/0001-profile-registry-layout.md), [ADR 0002 — Profile trust deferred](adr/0002-profile-trust-deferred.md).
