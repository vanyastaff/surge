# RFC-0006 · Agent Sandbox and Approvals

## Overview

The sandbox-and-approvals system is the **autonomy enabler**: it lets agents execute long-running work without per-action human approval, while preserving enough control that the user can safely walk away.

surge does **not** implement its own OS sandbox in v0.1. There is no Landlock layer, no AppContainer layer, no `sandbox-exec`, no custom DNS interception, and no filesystem policy engine owned by surge.

Instead, surge treats sandboxing as **agent-native capability**:

- Codex, Claude Code, Gemini CLI, and custom ACP agents already have their own permission/sandbox models.
- surge stores the desired agent launch configuration and sandbox intent in profiles and nodes.
- The ACP integration maps launch configuration and sandbox intent to the selected provider's supported session settings.
- If the provider requests permission/elevation, surge turns that request into a Telegram/UI approval event and resumes the agent with the user's decision when the provider supports that flow.

This keeps v0.1 focused on orchestration and remote control instead of rebuilding security primitives each agent runtime already ships.

## Core Principle

> The agent should be able to do everything its configured runtime sandbox allows without asking a human, and surge should ask the human only when the agent runtime requests a decision or the graph reaches a declared HumanGate.

This means:

- Agent runtime enforces **technical** boundaries.
- Profile/node config defines **orchestration policy**: launch mode, requested sandbox intent, approval policy, allowed MCP/tool exposure, max retries.
- HumanGate nodes are explicit pause points.
- Permission/elevation requests are rare exceptions that ping the user, not normal flow.

## What surge Owns

surge owns:

- Agent launch configuration model in graph/profile TOML.
- Sandbox intent model in graph/profile TOML.
- Provider capability detection and diagnostics.
- Mapping launch mode and sandbox intent to provider launch/session settings.
- Approval events, Telegram cards, UI/CLI resolution, audit log.
- "Allow once" / "Allow & remember" policy persistence.
- Secrets filtering before remote notifications.
- AGENTS.md trust and prompt-loading policy.
- Hooks triggered by engine lifecycle events.

surge does **not** own:

- OS-level filesystem enforcement.
- OS-level network enforcement.
- Kernel sandboxing.
- Shell subprocess containment.
- Provider-specific internals that are not exposed through ACP/CLI settings.

If a provider cannot enforce a requested mode, surge must say so clearly and either fail closed or require explicit user opt-in.

## Agent Launch Configuration

Launch configuration is the first layer of provider setup. It answers: **where and how is this agent session started?**

Most users should not write raw launch fields in every flow. They define named agents in `agents.yml`, then `flow.toml` nodes reference those names. This mirrors `docker-compose.yml`: the friendly file declares reusable services, and the generated graph references them.

```toml
[launch]
provider = "codex"                    # claude-code | codex | gemini | custom
mode = "local"                        # provider-default | local | cloud | sandbox
config_profile = "default"
extra_args = []
```

Launch modes are provider-native:

- **`provider-default`** — use the provider's default launch behavior.
- **`local`** — run the agent on the user's machine as a local process.
- **`cloud`** — use the provider's cloud/hosted execution mode when available.
- **`sandbox`** — use the provider's isolated/sandboxed execution target when available.

surge does not emulate a missing launch mode. If Codex supports a sandbox launch and Claude Code does not, the provider capability metadata says that clearly and validation handles it.

## Sandbox Intent Modes

These modes are portable intent labels. They are not a guarantee that every provider implements identical semantics.

### `provider-default`

Use the selected agent's default sandbox/permission behavior. This is useful during early integration or when the provider's defaults are already trusted by the user.

### `read-only`

Intent:

- Agent may inspect project files.
- Agent should not modify files.
- Agent should not run mutating commands.
- Network is disabled unless the provider default allows read-only network metadata.

Use case: planning agents, analyzers, reviewers.

### `workspace-write`

Intent:

- Agent may read the project.
- Agent may write within the run worktree.
- Agent may run normal build/test commands.
- Agent should not access user secrets or write outside the worktree.

Use case: implementers, test authors. This is the default intent for most coding nodes.

### `workspace+network`

Intent:

- Same as `workspace-write`.
- Agent may use network for declared development domains such as package registries, API docs, or GitHub.

Use case: dependency updates, codegen from public specs, package resolution.

### `full-access`

Intent:

- Agent runtime runs with broad access according to its own model.
- Used only for trusted stages such as PR composition or explicit user-approved operations.

`full-access` must never be silently selected by generated flows. It requires explicit profile/template configuration or a human approval.

### `custom`

Provider-specific options for users who know the selected runtime:

```toml
[sandbox]
mode = "custom"
provider = "codex"

[sandbox.provider_options]
approval_policy = "on-request"
network = "enabled"
extra_args = ["--some-provider-flag"]
```

Custom options are passed only to the matching provider. Unknown options are rejected during profile validation.

## Provider Capability Metadata

Each provider reports or is configured with capabilities:

```toml
[[providers]]
id = "codex"
supports_launch_modes = ["local", "cloud", "sandbox"]
supports_modes = ["read-only", "workspace-write", "workspace+network", "full-access"]
supports_permission_callbacks = true
supports_mcp_filtering = true
supports_network_policy = "provider-native"
notes = "Exact enforcement is owned by Codex."
```

Capability metadata is used by:

- `surge doctor`
- profile validation
- flow validation
- engine startup diagnostics
- Telegram warning cards when the requested policy is weaker/stronger than what the provider can express

## Profile Configuration

Profiles declare launch configuration and sandbox intent:

```toml
[launch]
provider = "codex"
mode = "local"
config_profile = "default"

[sandbox]
mode = "workspace-write"              # provider-default | read-only | workspace-write | workspace+network | full-access | custom
network_allowlist = ["crates.io", "github.com"]
require_provider_support = true

[approvals]
policy = "on-request"                 # untrusted | on-request | never
sandbox_approval = true
mcp_elicitations = false
request_permissions = true
skill_approval = false
elevation = true
```

`network_allowlist` is advisory unless the selected provider supports native network policy. If unsupported and `require_provider_support = true`, validation fails. If `false`, surge logs a warning and records the limitation in the run event log.

## Approval Policy

### Policy levels

- **`untrusted`** — conservative mode. Human approval is requested for generated flow approval, sensitive permission requests, and risky provider warnings.
- **`on-request`** — default. Agent runs autonomously inside its provider sandbox; user is pinged only for HumanGate nodes and provider permission/elevation requests.
- **`never`** — no approvals during this stage. Used for automated re-runs or trusted CI-like stages. If a provider asks for permission, the request is denied or the stage fails.

### Granular flags

| Flag | Effect when `true` |
|------|-------------------|
| `sandbox_approval` | Ask before accepting provider sandbox/elevation requests |
| `mcp_elicitations` | Forward MCP server elicitations to the user |
| `request_permissions` | Forward agent permission requests to the user |
| `skill_approval` | Ask before loading optional skills |
| `elevation` | Always ask before moving to `full-access` |

Combination: `policy = on-request, elevation = true` means the agent works autonomously until its runtime asks for more authority.

## Permission and Elevation Flow

When a provider exposes a permission/elevation request:

1. **Detection**: ACP bridge or provider adapter receives a permission request, tool approval request, or sandbox escalation signal.
2. **Event**: Engine writes `SandboxElevationRequested` or a more specific provider-permission event.
3. **Delivery**: Telegram bot sends a card with action, reason, stage, command/tool summary, and risk notes.
4. **Decision**:
   - `Allow once`: return approval to the provider for this request.
   - `Allow & remember`: approve and persist a broader profile/template policy when safe.
   - `Deny`: return denial to the provider.
5. **Resume**: Engine writes `SandboxElevationDecided` and resumes or fails the stage based on provider result.

If the provider cannot pause and wait for a decision, surge fails closed for risky requests unless the profile explicitly selected `full-access` or `provider-default`.

## Protected Paths and Secret Handling

Because surge does not own OS enforcement, protected-path handling is advisory unless supported by the provider. The engine still keeps a default deny/warning list:

```toml
deny = [
  "**/.env",
  "**/.env.*",
  "**/credentials.json",
  "**/secret*.{yml,yaml,json,toml}",
  "**/id_rsa*",
  "**/id_ed25519*",
  "**/.netrc",
]
```

Uses:

- Pass to providers that support path deny lists.
- Warn during flow/profile validation.
- Redact sensitive-looking data before Telegram notifications.
- Instruct bundled profiles to avoid copying secrets into summaries.

Telegram must never receive full source files, secret values, or raw environment dumps. It receives summaries, command snippets, metadata, and links to local views.

## Hooks

Hooks are user-defined shell commands executed at lifecycle points. They cannot be invoked by the agent directly; they are triggered by the engine.

### Hook triggers

- **`pre_tool_use`** — before a provider-visible tool call is accepted by the engine/adapter, when available.
- **`post_tool_use`** — after a tool returns successfully.
- **`on_outcome`** — after agent reports outcome, before routing.
- **`on_error`** — when stage fails or retry is attempted.

### Hook configuration

```toml
[[hooks.entries]]
trigger = "pre_tool_use"
matcher = 'tool == "edit_file"'
command = "./.surge/hooks/check_guard.sh ${TOOL_ARG_PATH}"
on_failure = "reject"                 # reject | warn | ignore
timeout_seconds = 10
inherit = "extend"                    # extend | replace | disable
```

Hook coverage depends on what the provider exposes. If a provider does not expose a tool event before execution, `pre_tool_use` cannot be guaranteed for that provider.

## AGENTS.md Rules Files

`AGENTS.md` is used for context injection and project rules. Loading scope hierarchy:

```text
1. ~/.surge/AGENTS.md
2. <profile>/<role>.md or .toml [prompt.system]
3. <project_root>/AGENTS.md
4. <subdir>/AGENTS.md
```

Rules are loaded in order, later scopes override earlier scopes. Subdirectory rules can be loaded just-in-time when the agent touches relevant paths, if the provider supports mid-session context injection. Otherwise they are loaded at stage start.

### Trust state

Rules files imported from external sources start untrusted. Untrusted files are not loaded into agent context until the user trusts them.

Trust is persisted globally:

```toml
[[trusted]]
path = "/home/user/projects/myapp/AGENTS.md"
hash = "sha256:..."
trusted_at = "2026-05-01T..."
```

## Notification Channels

Approvals can route through different channels in priority order:

```toml
[node_config.approvals]
channels = [
  { type = "telegram", chat_id_ref = "$DEFAULT" },
  { type = "desktop", duration = "persistent" },
  { type = "email", to_ref = "$USER_EMAIL" },
]
fallback_timeout_seconds = 1800
```

For the "user is away" scenario, Telegram is the primary channel. Desktop UI is a convenience, not a v0.1 dependency.

## Audit Log

Every approval event is in the run event log permanently:

- Who approved what, by channel.
- When it was approved.
- What context was shown.
- Whether policy changed because of "Allow & remember".
- Which provider launch and sandbox settings were requested and which were actually applied.

This is the source of truth for replay, debugging, and trust decisions.

## Security Model

surge's v0.1 security model is honest and limited:

- It is an orchestrator, not a sandbox kernel.
- It relies on the selected agent runtime for technical isolation.
- It records what it requested from the provider.
- It warns when the provider cannot honor the requested mode.
- It keeps code and secrets local; Telegram receives summaries only.

For high-risk repositories, users should run surge inside a VM, container, WSL distro, or dedicated machine and choose an agent provider with the sandbox guarantees they need.

## Acceptance Criteria

The agent sandbox and approval system is correctly implemented when:

1. Profiles can declare launch configuration and sandbox intent, and validation checks both against provider capability metadata.
2. Starting a session records requested and applied launch/sandbox settings in the event log.
3. Unsupported launch or sandbox modes fail with a clear diagnostic unless `provider-default` or explicit opt-in is selected.
4. A mock provider permission request reaches Telegram within 5 seconds.
5. Telegram `Allow once`, `Allow & remember`, and `Deny` decisions are written to the event log and returned to the mock provider.
6. "Allow & remember" persists a profile/template policy change and is visible in later validation.
7. Secret filtering catches common API key patterns before Telegram delivery.
8. `surge doctor` reports provider sandbox capabilities and warns about unsupported or unknown behavior.
9. End-to-end: a 30-minute autonomous run with default `workspace-write` produces zero approval requests beyond declared HumanGates and provider-native permission requests.
