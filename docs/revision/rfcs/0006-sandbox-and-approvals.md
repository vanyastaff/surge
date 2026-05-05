# RFC-0006 · Sandbox and Approvals

## Overview

The sandbox-and-approvals system is the **autonomy enabler**: it lets agents execute long-running work without per-action human approval, while preserving safety. Without effective sandboxing, the user's "describe and walk away" model collapses into per-tool-call confirmation dialogs.

This document specifies:
- Sandbox modes and what each allows/denies
- Per-OS sandbox enforcement mechanisms
- Approval policy and granular flags
- Hooks system
- AGENTS.md rules loading
- Trust state for projects and templates

## Core principle

> The agent should be able to do everything the **graph allows** without asking a human, and nothing else.

This means:
- Sandbox enforces **technical** boundaries (filesystem, network, processes)
- Profile/node config defines **policy** boundaries (allowed tools, max retries)
- HumanGate nodes are explicit pause points — the only place humans interact during a run
- Sandbox elevations are **rare exceptions** that ping the user, not normal flow

## Sandbox modes

Four built-in modes plus custom configurations:

### `read-only`

- Filesystem: read-only access to project worktree
- No network
- No subprocess execution
- No git operations

Use case: planning agents, analyzers, security auditors.

### `workspace-write`

- Filesystem: read everywhere allowed, write only within run worktree
- No network
- Subprocess execution allowed for declared shell allowlist (e.g., `cargo`, `git`)
- Git: read + commit allowed (within run branch), no push, no force operations

Use case: implementers, test authors. **Default for most coding agents.**

### `workspace+network`

- Same as `workspace-write` plus
- Network: outbound HTTPS to declared domain allowlist
- DNS: only allowed domains resolvable

Use case: agents that need to download dependencies (`cargo add` resolves crates.io), agents that fetch API specs.

### `full-access`

- Filesystem: full read/write (subject to `protected_paths` exclusions)
- Network: unrestricted
- Subprocess: unrestricted
- Git: full access including push

Use case: PR Composer (needs github push), CI integrations. **Always requires explicit elevation, never default.**

### Custom

A node can define its own sandbox profile:

```toml
[sandbox_override]
mode = "custom"
read = ["~/projects/**"]
write = ["~/.vibe/runs/<run_id>/**", "/tmp/vibe-*"]
network_allowlist = ["api.openai.com", "internal.company.com"]
shell_allowlist = ["docker", "kubectl"]
```

## Protected paths (always denied)

Regardless of sandbox mode, these are **always read-only or denied**:

- `.git/` — never write directly (use git commands)
- `.vibe/` — engine's own state
- `~/.ssh/`, `~/.config/`, `~/.aws/` — user secrets
- Files matched by `~/.vibe/global-deny.toml`

The default global deny list is shipped with the app and includes:

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

User can extend but not weaken this list.

## OS-level enforcement

The sandbox is enforced by the engine at multiple layers:

### Tier 1: MCP tool filtering (always active)

The engine controls which MCP tools the agent can call. Tools that would violate sandbox are not exposed at all. The agent literally doesn't see them in its tool list.

This is the primary enforcement layer.

### Tier 2: Filesystem path checking (always active)

For tools that take paths (`read_file`, `write_file`, `edit_file`):
- Engine intercepts the tool call before forwarding to MCP server
- Checks the path against allow/deny lists
- Rejects with informative error if violated

This catches cases where the agent tries to escape via path manipulation (e.g., `../../etc/passwd`).

### Tier 3: OS sandboxing (best effort, OS-dependent)

When available, the engine wraps shell subprocesses in OS-level sandboxes:

- **Linux**: Landlock (kernel 5.13+) for filesystem; seccomp-bpf for syscalls; or `nsjail` if installed.
- **macOS**: Seatbelt (`sandbox-exec`) with custom `.sb` profile
- **Windows**: Job Objects + AppContainer (more limited)

This prevents agents from escaping through subprocess spawn (e.g., `cargo build` → `build.rs` → arbitrary code).

If OS sandbox is unavailable, engine logs a warning and proceeds with Tier 1+2 only. User is notified at run start.

### Tier 4: Network isolation (best effort)

For `workspace+network` and `full-access`:
- DNS resolution restricted to allowlist (via `/etc/hosts` patching in nsjail, or DNS server override)
- Outbound connections monitored; deny attempts to reach non-allowlisted IPs

Like Tier 3, this is best-effort. On systems without nsjail, agents could theoretically bypass via direct IP calls, but this is impractical for typical agent workflows.

## Approval policy

Per-node approval policy:

```toml
[approvals]
policy = "on-request"                 # untrusted | on-request | never

# Granular flags
sandbox_approval = true               # ask before sandbox elevation
mcp_elicitations = false              # auto-confirm MCP server prompts
request_permissions = true            # ask if agent requests new perms
skill_approval = false                # auto-load safe skills
elevation = true                      # always ask before full-access
```

### Policy levels

- **`untrusted`** — every meaningful action requires approval. Used when running untrusted templates or third-party flows.
- **`on-request`** — agent runs autonomously within sandbox, approvals requested only for elevation events. **Default for normal use.**
- **`never`** — no approvals during this stage. Used for automated re-runs, CI integrations.

### Granular flags

Override the policy for specific event types:

| Flag | Effect when `true` |
|------|-------------------|
| `sandbox_approval` | Ask before commands escape sandbox |
| `mcp_elicitations` | MCP server can ask agent for input → forwarded to user |
| `request_permissions` | Agent can request new tool/scope, ask user |
| `skill_approval` | New skills require approval before use |
| `elevation` | Always ask before sandbox raised to higher tier |

Combination: `policy = on-request, elevation = true` (default) means: agent works autonomously, only pings for sandbox tier upgrades.

## Sandbox elevation flow

When an agent attempts an action outside its sandbox:

1. **Detection**: Tool filter / path checker / OS sandbox catches the attempt.
2. **Decision**: Engine checks node's `approvals.elevation`:
   - `false` (auto-deny): write `SandboxElevationDecided { decision: deny }`, agent receives error.
   - `true` (ask): write `SandboxElevationRequested`, send Telegram card, pause.
3. **User response**: 
   - `Allow once`: action proceeds, no template change.
   - `Allow & remember`: action proceeds, template's allowlist is permanently extended.
   - `Deny`: action fails with informative error to agent.
4. **Resolution**: write `SandboxElevationDecided`, resume execution.

The "remember" option makes the system **learn from user choices**. Each future run from the same template starts with broader allowlist, fewer pings.

## Hooks

Hooks are user-defined shell commands executed at lifecycle points. They cannot be invoked by the agent directly — they're triggered by the engine.

### Hook triggers

- **`pre_tool_use`** — before agent calls a tool
- **`post_tool_use`** — after tool returns successfully
- **`on_outcome`** — after agent reports outcome (before engine routes)
- **`on_error`** — when stage fails or retry attempted

### Hook configuration

```toml
[[hooks.entries]]
trigger = "pre_tool_use"
matcher = 'tool == "edit_file"'       # JS-like predicate
command = "./.vibe/hooks/check_guard.sh ${TOOL_ARG_PATH}"
on_failure = "reject"                 # reject | warn | ignore
timeout_seconds = 10
inherit = "extend"                    # extend | replace | disable
```

### Available context variables in hook commands

- `${RUN_ID}` — current run ID
- `${NODE_ID}` — current node
- `${TOOL_NAME}` — for tool hooks
- `${TOOL_ARG_*}` — tool argument values (e.g., `${TOOL_ARG_PATH}`)
- `${OUTCOME_ID}` — for `on_outcome` hooks
- `${WORKTREE}` — path to run's worktree

### Hook on_failure semantics

- **`reject`**: stage fails as if hook is part of the contract
- **`warn`**: log a warning, continue
- **`ignore`**: silent, continue

For `on_outcome` hooks, `reject` causes outcome rejection (covered in RFC-0003): retry counter increments, agent gets another chance.

### Hook inheritance

Hooks combine across scopes (global → profile → project → node):

- **`extend`** (default): node-level hooks add to inherited hooks
- **`replace`**: node-level hooks replace inherited hooks at this trigger
- **`disable`**: explicitly disable specific inherited hooks by ID

This lets profiles ship reasonable defaults while allowing per-node opt-out.

## AGENTS.md rules files

`AGENTS.md` is the de facto standard for AI coding agent rules (Linux Foundation initiative). vibe-flow uses this format for context injection.

### Loading scope hierarchy

```
1. ~/.vibe/AGENTS.md                          (global, user-level)
2. <profile>/<role>.md or .toml [prompt.system] (profile-level)
3. <project_root>/AGENTS.md                   (project-level)
4. <subdir>/AGENTS.md                         (subdir-level, JIT)
```

Loaded in order, later overrides earlier. JIT (just-in-time) loading: subdir AGENTS.md is loaded only when an agent touches a file in that subdir, saving tokens.

### Rule file format

Standard markdown. Front matter optional for metadata:

```markdown
---
applies_to: ["**/*.rs"]
priority: high
---

# Project rules

- All public functions must have rustdoc comments
- No `unwrap()` outside `#[cfg(test)]`
- Use `tracing` not `log` for logging
```

The `applies_to` pattern (if specified) restricts the rule to matching files. The agent sees rules only when they apply.

### Trust state

A rules file can be marked untrusted:

- Files imported from external sources start untrusted
- User must explicitly trust them in the editor
- Untrusted files are NOT loaded into agent context, regardless of scope

Trust is per-file, persisted in `~/.vibe/trust.toml`:

```toml
[[trusted]]
path = "/home/user/projects/myapp/AGENTS.md"
hash = "sha256:..."
trusted_at = "2026-05-01T..."

[[untrusted]]
path = "/home/user/projects/myapp/imported/external-rules.md"
reason = "Imported from external source, awaiting review"
```

### Project trust

A project (working directory) is also trusted/untrusted:

- New project (first time `vibe run` is invoked): prompts for trust decision
- Trusted: project's `AGENTS.md` and `.vibe/` configs are loaded
- Untrusted: project hooks, rules, custom node types are skipped (Codex-style)

Trust is project-path-based, persisted globally.

## JIT context loading

For large projects with many AGENTS.md files in subdirs:

### Strategy

Engine maintains a watch list of subdir AGENTS.md files (just paths, not content). When the agent reads or writes a file in a subdir, engine:

1. Checks if there's an unloaded AGENTS.md in that subdir or its parents up to the loaded root.
2. If yes, reads it, validates trust, prepends to next agent turn's context.
3. Tracks "loaded" status to avoid re-reading.

### Token budget

Each profile has a `max_context_tokens` that JIT loading respects. If loading the new AGENTS.md would exceed budget, the engine either:
- Skips loading (and logs warning)
- Truncates oldest non-essential context to make room

User-configurable via `auto_trim_oldest = true` (default).

### Disabling JIT

Set `[runtime].load_rules_lazily = false` in profile to load all AGENTS.md upfront. Useful for short runs where the up-front cost is acceptable and avoiding mid-run context shifts.

## Notification channels (recap from RFC-0001 + sandbox angle)

Approvals can route through different channels in priority order:

```toml
[node_config.approvals]
channels = [
  { type = "telegram", chat_id_ref = "$DEFAULT" },
  { type = "desktop", duration = "persistent" },
  { type = "email", to_ref = "$USER_EMAIL" },     # fallback
]
fallback_timeout_seconds = 1800                    # try next channel after 30 min
```

Critical for "user is away" scenarios: if Telegram delivery fails or user doesn't respond in 30 min, engine escalates to email or desktop notification.

## Audit log

Every approval event is in the run's event log permanently. This is the audit trail:

- Who approved what (channel + decision)
- When (seq + timestamp)
- What was the context (the artifact being approved)
- What changed in trust state ("Allow & remember" → which template extended)

For compliance-conscious users, this gives a complete record. Future enterprise feature could export this to SIEM, but for v1 it's just the SQLite event log.

## Open questions

### Network egress monitoring

Even with allowlist, an agent could exfiltrate data via DNS queries to allowed domains (e.g., encoding data in subdomain queries to a domain that has wildcard support). This is theoretical — in practice, mitigation:

- Log all DNS queries
- Pattern-match for suspicious patterns (high entropy, unusual subdomain structure)
- Per-template "data egress concerns" flag that tightens monitoring

For v1: log + pattern-match + warn. Active blocking is v2.

### Sandbox escape via OS bugs

The agent could find a kernel bug (in Landlock, seccomp) and escape. Mitigation: defense in depth — multiple tiers, with Tier 1 (MCP filtering) being the strongest. Even if Tier 3 is bypassed, the agent has limited tool access.

For high-stakes work (production systems), users should run vibe-flow in a VM or container. Document this in security best practices.

### Approval forgery

A malicious actor with access to user's Telegram could approve runs. Mitigations:
- Two-factor approval for elevation events (Telegram + desktop confirmation)
- Approval signing (require user to type a confirmation phrase for sensitive operations)

For v1: Telegram-only. Two-factor is opt-in v2.

## Acceptance criteria

The sandbox and approval system is correctly implemented when:

1. An Implementer node with default `workspace-write` sandbox cannot write outside the worktree (verified via attempted escapes in test).
2. Trying to access `~/.ssh/` from any sandbox mode is denied at Tier 1 + Tier 2 + Tier 3.
3. Sandbox elevation request reaches Telegram within 5 seconds of the agent's blocked attempt.
4. "Allow & remember" persists the new allowlist entry in the template, verified by inspecting template TOML after run.
5. AGENTS.md JIT loading saves measurable tokens vs. eager loading on a 100-file project (test fixture).
6. Untrusted AGENTS.md files are NOT included in agent context, verified by token-level inspection.
7. Hooks fire reliably and reject failed outcomes per `on_failure` policy.
8. OS sandbox enforcement works on Linux (Landlock available), macOS (sandbox-exec), and Windows (Job Objects).
9. End-to-end: a 30-minute autonomous run with default `workspace-write` produces zero approval requests beyond the 3 declared HumanGates.
