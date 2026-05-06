# RFC-0005 · Profiles and Roles

## Overview

A **profile** is a reusable configuration for an Agent node. It encapsulates everything that defines an agent's role: system prompt, allowed tools, agent launch configuration, agent-native sandbox intent, default outcomes, recommended model. Profiles are TOML files in `~/.surge/profiles/` (and shipped defaults).

A **role** is the user-facing concept: "Implementer", "Reviewer", "Spec Author". Each role corresponds to one (or more, versioned) profile.

This document specifies:
- Profile file format
- Role inheritance
- Bundled v1 roles
- How profiles integrate with node configuration
- Versioning and discovery

## Why profiles are first-class

Without profiles, every Agent node would carry its full configuration inline (system prompt, tools, launch settings, sandbox intent, etc.) — duplicated across runs and projects. Updating the "Implementer" prompt would require touching every pipeline. Profiles solve this:

- **DRY**: One profile, many nodes referencing it
- **Versioning**: Profile updates propagate to nodes via semver
- **Discoverability**: Flow Generator picks profiles from registry by metadata
- **Customization**: Per-node overrides for special cases without forking the profile

## Profile file format

```toml
schema_version = 1

[role]
id = "implementer"                    # stable kebab-case
version = "1.0"                       # semver
display_name = "Implementer"
icon = "⊙"                            # for UI
category = "agents"                   # agents | gates | flow | io | _bootstrap
description = "Writes Rust code following plan.md and spec.md"
when_to_use = "Standard implementation work where plan and spec exist"

[runtime]
recommended_model = "claude-opus-4-7"
default_temperature = 0.2
default_max_tokens = 200000

[launch]
provider = "claude-code"              # claude-code | codex | gemini | custom
mode = "local"                        # provider-default | local | cloud | sandbox
config_profile = "default"            # provider-specific launch/config profile
extra_args = []                       # provider-specific CLI/ACP args

[sandbox]
default_mode = "workspace-write"      # provider-default | read-only | workspace-write | workspace+network | full-access
default_writable_roots = []           # advisory unless provider supports path policy
default_network_allowlist = ["crates.io", "github.com", "*.githubusercontent.com"]
default_protected_paths = [".git", ".surge", "~/.ssh", "~/.config"]

[tools]
default_mcp = ["filesystem", "shell", "git"]
default_skills = ["rust-expert"]
default_shell_allowlist = ["cargo", "rustc", "rustfmt", "clippy"]

[approvals]
policy = "on-request"                 # untrusted | on-request | never
sandbox_approval = true
mcp_elicitations = false
request_permissions = true
skill_approval = false
elevation = true                      # always ask before full-access

[[outcomes]]
id = "done"
description = "All planned changes committed, build succeeds, tests pass"
edge_kind_hint = "forward"
required_artifacts = ["**/*.rs"]      # at least one source file changed

[[outcomes]]
id = "blocked"
description = "Plan/spec contradicts code reality, need replanning"
edge_kind_hint = "backtrack"

[[outcomes]]
id = "escalate"
description = "Architectural decision needed from human"
edge_kind_hint = "escalate"

[bindings]
# Default expected inputs (Flow Generator wires these up)
expected = [
  { name = "spec", source = "node_output", from_role = "spec-author" },
  { name = "plan", source = "node_output", from_role = "architect" },
  { name = "adrs", source = "node_output", from_role = "architect", optional = true },
]

[hooks]
# Default lifecycle hooks
[[hooks.entries]]
trigger = "post_tool_use"
matcher = 'tool == "edit_file"'
command = "cargo fmt -- --check $FILE"
on_failure = "warn"

[[hooks.entries]]
trigger = "on_outcome"
matcher = 'outcome == "done"'
command = "cargo test --lib && cargo clippy -- -D warnings"
on_failure = "reject_outcome"         # rejecting forces retry

[prompt]
system = """
You are an expert Rust implementer. Modern idioms (Rust 1.75+).
Prefer narrow trait objects over `dyn Any`. No `unwrap()` outside tests.
Document every public item with `///`.

# Inputs available
{{#if spec}}- Spec: see {{spec}}{{/if}}
{{#if plan}}- Plan: see {{plan}}{{/if}}
{{#if adrs}}- ADRs: see {{adrs}}{{/if}}

# Your task
Implement the plan in {{plan}} against the spec in {{spec}}. Honor every ADR.

# Constraints
- All public APIs must have rustdoc comments
- No `unwrap()` outside `#[cfg(test)]` modules
- New dependencies require justification in commit message

# When you're done
Call `report_stage_outcome` with one of:
- "done" — all changes committed, `cargo test --lib` passes, `cargo clippy` clean
- "blocked" — found plan/spec ambiguity that requires replanning
- "escalate" — architectural decision needed (specify what in summary)
"""

[inspector_ui]
# Custom fields shown in node config inspector for this role
[[inspector_ui.fields]]
id = "max_files_per_attempt"
label = "Max files modified per attempt"
kind = "number"
default = 20
help = "If exceeded, agent must split into multiple attempts"

[[inspector_ui.fields]]
id = "require_tests"
label = "Require new tests for new code"
kind = "toggle"
default = true
```

## Field reference

### `[role]`

- `id` — stable identifier, kebab-case. Profile is referenced as `id@version`.
- `version` — semver. Major bump = breaking changes.
- `display_name` — human-readable name shown in editor and Telegram.
- `icon` — single character or short string for canvas icon.
- `category` — affects sidebar grouping in editor.
- `description` — short explanation for end users.
- `when_to_use` — guidance for Flow Generator's selection logic.

### `[runtime]`

Sets model and generation defaults for the agent invocation. Per-node overrides exist.

### `[launch]`

Defines how the agent is started when no named agent is selected from `agents.yml`. `provider` selects the agent family, `mode` selects the provider-supported execution target (`local`, `cloud`, `sandbox`, or provider default), and `config_profile`/`extra_args` pass through provider-specific launch configuration. This is separate from `[sandbox]`: launch mode says where/how the agent runs, sandbox says what permissions it should request inside that run.

Launch config is resolved per node. Profile defaults are only defaults: a flow can select a named agent from `agents.yml` or override the provider/launch mode on a specific Agent node when the user wants different agents for different stages, such as Claude for implementation and Codex or Gemini for review.

### `[sandbox]`

Default sandbox intent. Maps to RFC-0006 agent-native sandbox modes and is passed to the selected provider when supported. Per-node overrides are allowed for power users.

### `[tools]`

Lists of MCP servers and skill folders the agent has access to. Empty list = no tools of that kind.

### `[approvals]`

Default approval policy and granular flags (matches the segmented control in node config). Per-node override possible.

### `[[outcomes]]`

Declared outcomes the agent can report. The agent's `report_stage_outcome` tool will reject any ID not in this list. `required_artifacts` can specify glob patterns that must have been produced for that outcome to be valid.

### `[bindings]`

Hints for Flow Generator about what inputs this role typically expects. Used to wire up edges automatically when this profile is added to a graph.

### `[hooks]`

Default lifecycle hooks. These are appended to any hooks defined at the project level. Per-node config can add or disable specific hooks.

### `[prompt]`

The system prompt template. Uses Handlebars-like syntax for variable interpolation:
- `{{name}}` — direct substitution
- `{{#if name}}...{{/if}}` — conditional block
- `{{#each list}}...{{/each}}` — iteration

Template variables are populated from bindings at execution time.

### `[inspector_ui]`

Definitions of custom fields shown in the node Inspector when a node uses this profile. These fields become part of the node's config and are passed to the agent as additional context.

## Profile resolution

When a node references `profile = "implementer@1.0"`:

1. Engine looks up `~/.surge/profiles/implementer-1.0.toml` (most specific path)
2. Falls back to `~/.surge/profiles/implementer.toml` (latest version) with semver compatibility check
3. Falls back to bundled profile in app distribution
4. Errors out if not found

For semver constraints (`implementer@^1.0`):
1. Find all installed profiles matching `implementer-*.toml`
2. Filter by semver match
3. Select highest matching version

This is similar to Cargo's resolution.

## Profile inheritance

A profile can inherit from another:

```toml
[role]
id = "rust-implementer"
version = "1.0"
extends = "generic-implementer@1.0"

[runtime]
# Override only what differs
recommended_model = "claude-opus-4-7"

[tools]
default_skills = ["rust-expert"]      # extends parent's list

[prompt]
system = """
{{> base}}                            # inserts parent's prompt

# Rust-specific additions
- Use Rust 1.75+ idioms
"""
```

Inheritance is **shallow merge**: child overrides parent for each key, except for list fields where the child can specify `extends_list = true` to append rather than replace.

## Bundled v1 profiles

Shipped in `surge/profiles/` directory of the repo, installed to `~/.surge/profiles/` on first run.

### Bootstrap profiles (under `_bootstrap/`)

- **`description-author@1.0`** — Stage 1 of bootstrap
- **`roadmap-planner@1.0`** — Stage 2 of bootstrap
- **`flow-generator@1.0`** — Stage 3 of bootstrap

These are special — `category = "_bootstrap"`, hidden from user node library by default.

### Standard agent profiles

- **`spec-author@1.0`** — Reads description, produces formal spec.md with API surface, edge cases, non-goals.
- **`architect@1.0`** — Reads spec, produces plan.md with module breakdown, file tree, ADRs as needed.
- **`implementer@1.0`** — Writes code per plan, ensures tests pass before reporting done.
- **`test-author@1.0`** — Writes tests against spec (TDD-style, before implementation).
- **`verifier@1.0`** — Runs full test suite, clippy, doc check; reports pass/fail.
- **`reviewer@1.0`** — Reads diff, identifies logic errors, architecture issues, missed edge cases.
- **`pr-composer@1.0`** — Composes PR description from run history, opens PR via GitHub MCP.

### Specialized profiles (optional, ship in v0.2)

- **`bug-fix-implementer@1.0`** — Implementer variant focused on minimal changes
- **`refactor-implementer@1.0`** — Implementer variant with diff-min discipline
- **`security-reviewer@1.0`** — Reviewer focused on auth, input validation, secrets
- **`migration-implementer@1.0`** — Implementer for dependency upgrades

## Profile registry CLI

```
surge profile list                     # list all installed profiles
surge profile show <id>                # show profile details
surge profile install <path-or-url>    # install a profile from file or URL
surge profile uninstall <id>           # remove a profile
surge profile validate <path>          # check profile is valid
surge profile diff <id> <id>           # compare two versions
```

## Profile authoring guidelines

For users (or community) writing new profiles. Anti-patterns to avoid:

### Don't make per-language variants

Bad:
```
rust-implementer
python-implementer
typescript-implementer
```

Good:
```
implementer (with {{language}} template variable, auto-detected from project)
```

Exceptions: when the role is fundamentally language-specific (e.g., `rust-expert-skill` is a skill, not a profile). For roles that are language-agnostic in concept (Implementer, Reviewer, Verifier), use template variables.

### Don't make trivial role splits

Bad: `bug-fix-reviewer` that has 80% same prompt as `reviewer`.

Good: One `reviewer` profile with a `review_focus` config field that adjusts behavior:
```toml
[inspector_ui]
[[inspector_ui.fields]]
id = "review_focus"
kind = "select"
options = ["general", "logic", "security", "perf"]
default = "general"
```

### Don't tie profiles to specific tools

Bad: profile system prompt that says "use ripgrep for searching".

Good: profile says "search the codebase using available tools". Tool availability is configured separately, prompt remains tool-agnostic.

### Make outcomes meaningful

Bad: outcomes `done`, `not_done` (the second is meaningless).

Good: outcomes `done`, `blocked` (with reason), `escalate` (with question for human). Each outcome corresponds to a real semantic state the engine needs to route differently.

### Test the prompt

Every profile should have a `tests/` subfolder with example invocations:
```
profiles/implementer/
├── implementer-1.0.toml
└── tests/
    ├── basic-impl.input.json     # mock inputs
    ├── basic-impl.expected.json  # expected outcome behavior
    └── ...
```

CI runs these against the LLM and verifies outcomes match. Without this, prompts drift silently.

## Versioning policy

- **Patch (1.0.0 → 1.0.1)**: prompt rewording without behavior change, new examples, bug fixes in template logic. Backward-compatible.
- **Minor (1.0.x → 1.1.0)**: new optional outcomes, new optional bindings, expanded tool defaults. Backward-compatible.
- **Major (1.x → 2.0.0)**: removed outcomes, changed required bindings, incompatible prompt rewrites. Breaking.

Pipelines should reference profiles with caret semver (`^1.0`) for auto-updates within minor versions, or pinned (`1.0.3`) for reproducibility.

## Acceptance criteria

The profile system is correctly implemented when:

1. All 7 v1 profiles validate, load, and produce coherent agent behavior on test inputs.
2. A new profile (e.g., `linter@1.0`) can be added by dropping a TOML file in `~/.surge/profiles/` with no code changes; it appears in the editor's Node Library and is selectable by Flow Generator.
3. Profile inheritance works correctly: a child profile gets parent's defaults plus its own overrides.
4. Per-node overrides (e.g., overriding `default_temperature`, launch provider, or launch mode) take precedence over profile defaults.
5. `surge profile diff implementer@1.0 implementer@1.1` shows a clear comparison of changed fields.
6. Flow Generator's prompt receives the profile registry as structured context and selects appropriate profiles for each milestone/task.
7. Inspector UI custom fields render correctly in the node config dialog when a node uses a profile that defines them.
