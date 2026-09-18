+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-09-18"
+++

# ADR 0019 — Providers are registry data, not code

## Status

Accepted.

## Context

Surge's pitch is agent-agnosticism: any ACP-conformant agent should work, and
the operator should be able to add one without editing surge. The v0.1 shape
had two gaps against that promise:

1. **The engine resolved agent ids through `Registry::builtin()` only.**
   `derive_agent_kind_from_id` hard-coded the builtin catalog, so an
   `[agents.*]` entry in `surge.toml` was reachable from the legacy pool
   (`surge prompt`, `surge agent test`, the desktop chat) but **never from a
   run** — `surge engine run` and the daemon would report the agent id as
   unknown. The documented "custom provider" path silently did not work
   where it mattered most.
2. **A provider's environment and startup files had no data shape.** An agent
   that needed env vars or a project-local settings file could only get them
   by hand-editing the process environment before launching surge, and the
   one piece of launch-time file materialisation that existed (the Claude
   Code headless permission seed) was a vendor branch keyed on
   `RuntimeKind::ClaudeCode`.

Adding a direct HTTP client to a provider's API was considered and rejected:
it would reopen [ADR-0006](0006-acp-only-transport.md) and replace the ACP
transport with a per-provider integration — the parser-maintenance treadmill
in a different costume.

## Decision

**A provider is a registry entry; the launch contract is data.** The engine
resolves a profile's `runtime.agent_id` through one unified catalog — the
operator's `[agents.*]` first, then the builtin entries — and the entry's
`command`, `default_args`, `env`, and `settings_files` are the whole launch
contract. There is no per-vendor branch anywhere:

- `surge_core::config::AgentConfig` and `surge_acp::RegistryEntry` carry
  `env: BTreeMap<String, AgentEnvValue>` and `settings_files: Vec<AgentSettingsFile>`.
- `AgentEnvValue` is either a literal or an injection of the operator's
  environment by variable **name** (`{ from = "OLLAMA_API_KEY" }`); a
  required-but-unset source fails the launch with an error naming the
  variable, never a value.
- `AgentSettingsFile` declares a worktree-relative file (path + content) to
  materialise when absent. `surge_acp::settings_seed` writes it generically;
  an existing file is never clobbered and absolute/`..` paths and symlinked
  parents are refused.
- `Registry::for_run(&SurgeConfig)` builds the unified catalog; the CLI, the
  daemon, and the legacy pool all construct it. Wrapper entries (npx/uvx)
  resolve to the verbatim-spawn `AgentKind::Custom` path **by shape**, not by
  id.

## Rationale

1. **It is the promise, made testable.** "Pick an agent from the catalog or
   declare your own" is only true if both take the same code path. One
   catalog, one resolver, one spawn builder is what makes a new provider
   cost zero lines of surge code.
2. **Secrets stay out of surge.** Name indirection is the existing convention
   (`[[task_sources]] api_token_env`, `[telegram] bot_token_env`); reusing it
   for provider credentials means `surge.toml`, the event log, and the config
   surface never carry key material.
3. **Startup files are an agent fact, not a vendor fact.** Which file an
   agent reads at session setup is the agent's business; declaring it as
   data removes the last `RuntimeKind` branch from the launch path and
   extends automatically to providers surge has never seen.
4. **ADR-0006 is preserved.** Surge still speaks ACP only. Provider routing
   (a base URL, a model name, an API key) lives in the child's environment,
   which is exactly the "subscription or API key, the runtime's own choice"
   posture ADR-0006 describes — now expressible without a code change.

## Consequences

- `RuntimeKind` becomes purely descriptive (sandbox-matrix lookup, doctor
  reporting) and is **not** consulted for launch behaviour. New providers do
  not need a variant; they need an entry.
- The Ollama provider ships as the worked example: `ollama-acp` is pure JSON
  (npx adapter, `ANTHROPIC_*` env spec, Claude settings seed), and
  `ollama-verifier@1.0` is a bundled cross-vendor verifier on it.
- Engine wiring must pass the merged registry (`EngineConfig::agent_registry`).
  A caller that leaves it `None` gets `Registry::builtin()` — the previous
  behaviour, which keeps tests and legacy callers working unchanged.
- A user entry with a builtin id overrides the builtin entry. That is
  intentional (an operator redirecting `claude-acp` to their own wrapper is
  legitimate), and it is the one way a builtin can be shadowed.

### Accepted costs and mitigations

| Cost | Mitigation |
|---|---|
| A merged registry adds one construction point to keep wired | `Registry::for_run` is the single constructor; CLI, daemon and legacy pool call it, and a `None` fallback preserves old behaviour |
| `env` literals can tempt someone to paste a secret into `surge.toml` | Validation warns when a literal matches the secret-redaction patterns; the docs and the `surge.example.toml` comment say "use `from`" |
| `settings_files` content is embedded in the registry JSON | It is a startup file, not a secret; the seed refuses path escapes and symlinked parents, and never clobbers |

## Out of scope

- **Direct LLM HTTP clients.** Unchanged from ADR-0006: surge does not call a
  provider's completion API; it drives a coding agent over ACP.
- **Sandbox-tier launch flags.** The delegation matrix's `flags` still do not
  reach any production spawn (a pre-existing gap, not created or closed
  here). This ADR does not claim otherwise, and `ollama-acp`'s matrix rows
  stay declared-unverified.
- **Injected-tool transport to real agents.** `report_stage_outcome` is
  declared to the bridge but not transported to a real ACP agent in the
  pinned SDK. Pre-existing; tracked separately.

## Revisit conditions

- A provider appears that cannot be expressed as `command`/`args`/`env`/
  `settings_files` — then the data shape is wrong, not the principle.
- The SDK grows client-provided tool declarations, which would let injected
  tools travel over ACP and make the injected-tool gap closable inside
  ADR-0006.
