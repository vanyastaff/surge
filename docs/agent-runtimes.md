# Agent runtimes

Surge is agent-agnostic: it talks to any coding agent over Agent Client Protocol (ACP) (see
[ADR-0006](adr/0006-acp-only-transport.md)). This page is the per-runtime
support matrix — how each runtime is launched, its current validation status,
and known quirks.

## Providers are data, not code

There is no per-vendor branch anywhere in surge. A provider is a **registry
entry** — `command`, `default_args`, `env`, `settings_files` — and the engine
resolves a profile's `runtime.agent_id` through one unified catalog: the
operator's `[agents.*]` from `surge.toml` first, then the builtin entries.
Adding a provider is adding an entry; adding one the operator invents needs
no surge change at all:

```toml
[agents.my-provider]
command = "npx"
args = ["-y", "some-acp-agent"]
transport = "stdio"
capabilities = ["code", "chat"]

[agents.my-provider.env]
MY_PROVIDER_BASE_URL = { from = "MY_PROVIDER_URL", required = true }
MY_PROVIDER_API_KEY = { from = "MY_PROVIDER_KEY", required = true }
```

A profile naming `agent_id = "my-provider"` then launches it, from
`surge engine run`, the daemon, `surge prompt`, `surge agent test` and the
desktop chat alike. `env` values are resolved at spawn time from the
operator's environment — **a `from` spec carries the variable name, never the
secret**; a required-but-unset variable fails the stage with an error naming
it. `settings_files` materialises files an agent needs in its worktree before
startup (e.g. a non-interactive permission mode), and an existing file is
never clobbered.

## Support matrix

| Runtime | Registry id | Launch (registry / npx) | Direct-CLI invocation | Live-validated |
|---|---|---|---|---|
| Claude Code | `claude-acp` | `npx @zed-industries/claude-agent-acp` | `claude --acp` | ✅ (v0.1, end-to-end) |
| Codex | `codex-acp` | `npx @zed-industries/codex-acp` | `codex acp` | ⬜ wired + arg-tested; operator-pending |
| Gemini CLI | `gemini` | `npx @google/gemini-cli --acp` | `gemini --acp` | ⬜ wired + arg-tested; operator-pending |
| GitHub Copilot | `github-copilot-cli` | `npx @github/copilot --acp` | (via npx) | ⬜ wired; operator-pending |
| Ollama (local or cloud) | `ollama-acp` | `npx -y @zed-industries/claude-agent-acp` + `ANTHROPIC_*` routing | (via npx) | ✅ transport handshake → prompt → reply, live |
| DeepSeek Harness (**developer preview**) | `dsh-acp` | `npx -y @deepseek-ai/dsh --profile acp` | (via npx) | ⬜ wired + arg-tested; scheduled CI canary only (`.github/workflows/dsh-canary.yml`: a hard pin+handshake gate plus a soft full-flow step that needs model credentials), not the PR gate |

The registry ([`crates/surge-acp/builtin_registry.json`](../crates/surge-acp/builtin_registry.json))
is the default launch path: `command = "npx"` and `default_args` already encode
the complete invocation (including any `--acp` flag). Those entries resolve to
`AgentKind::Custom` and are spawned verbatim — the builder injects nothing.
A wrapper entry (npx/uvx) always takes that verbatim path regardless of its
id, so a user-declared provider cannot be mis-launched by the typed arms.

The typed `AgentKind` arms (`ClaudeCode` / `Codex` / `GeminiCli`) are the
**direct-CLI** model (a real `claude`/`codex`/`gemini` on `PATH`), exercised by
the env-gated `real_acp_smoke` test. There the builder injects the runtime's
ACP entrypoint:

- Claude / Gemini take a `--acp` **flag**.
- Codex takes a bare `acp` **subcommand** (not `--acp`) — a common foot-gun;
  pinned by the launch-arg unit tests in `crates/surge-acp/src/bridge/worker.rs`.

Binary resolution is PATHEXT-aware (`which`), so bare `npx` resolves to the
`.cmd`/`.bat` shim on Windows.

## Ollama

Ollama exposes an Anthropic-compatible endpoint, and
`@zed-industries/claude-agent-acp` drives Claude Code over ACP against it.
The `ollama-acp` builtin entry is pure data:

```json
"env": {
  "ANTHROPIC_BASE_URL":  { "from": "OLLAMA_HOST", "required": true },
  "ANTHROPIC_AUTH_TOKEN":{ "from": "OLLAMA_API_KEY", "default": "ollama", "required": false },
  "ANTHROPIC_API_KEY":   "",
  "ANTHROPIC_DEFAULT_SONNET_MODEL": { "from": "OLLAMA_MODEL", "required": true },
  "ANTHROPIC_DEFAULT_OPUS_MODEL":   { "from": "OLLAMA_MODEL", "required": true },
  "ANTHROPIC_DEFAULT_HAIKU_MODEL":  { "from": "OLLAMA_MODEL", "required": true }
}
```

Cloud (`OLLAMA_HOST=https://ollama.com`, key from
<https://ollama.com/settings/keys>) or local
(`OLLAMA_HOST=http://localhost:11434`, no key). `OLLAMA_MODEL` names a model
from <https://ollama.com/search> (e.g. `glm-5.3`); it is **required** — a
launch without it fails loudly rather than silently reaching Anthropic. The
key value never enters surge: the spec carries only the variable name.

The live test is opt-in and transport-level (spawn → handshake → prompt →
non-empty reply):

```sh
OLLAMA_HOST=https://ollama.com OLLAMA_API_KEY=... OLLAMA_MODEL=glm-5.3 \
  cargo nextest run -p surge-acp --test ollama_acp_smoke --run-ignored ignored-only
```

`ollama-verifier@1.0` (bundled) is a sealed, authority-bearing verifier on
this runtime, so a Claude/Codex implementer's flow can verify cross-vendor.

## Auth

Each runtime authenticates on its own (its native `login`, an API-key env
var, or — for a provider declared in `surge.toml` — the `env` spec above).
When a launched agent fails to authenticate, the prompt-dispatch error is
classified as `SendMessageError::AgentAuthenticationFailed` (see
[ADR-0006](adr/0006-acp-only-transport.md) and PR #71) and surfaces to the
operator with actionable guidance ("verify the agent runtime is logged in …").
This classification is runtime-agnostic and applies to every runtime above.

## Headless / permission mode

For unattended runs, the agent must not block on interactive approval prompts:

- An agent whose startup aborts on a globally-configured mode declares its
  project-local fix as data: `settings_files` on its registry entry (or
  `[agents.<id>]` in `surge.toml`). The builtin Claude-agent and Ollama
  entries ship `.claude/settings.json` with
  `permissions.defaultMode = "default"`; the seed never clobbers an existing
  file, and `surge doctor agent <id>` applies it before its real smoke.
- **Codex / Gemini / Copilot**: the equivalent headless setting is TBD during
  live validation — record it here once confirmed, and add it as a
  `settings_files` entry if it is a file.

## Polling cadence

Tracker polling cadence is per-source and tier-aware (L1 5min / L2 2min /
L3 1min, with rate-limit backoff) for the **GitHub** source; see
[`tracker-automation.md`](tracker-automation.md). This is independent of the
agent runtime.

## Validating a runtime live

Surge cannot validate Codex/Gemini/Copilot in CI (the binaries + auth aren't
present). To validate a runtime on a machine where it is installed and
logged in:

```sh
# Full end-to-end flow against a real ACP agent:
SURGE_REAL_ACP_BIN=/path/to/codex \
SURGE_REAL_ACP_KIND=codex \
SURGE_REAL_ACP_PROFILE=implementer@1.0 \
  cargo test -p surge-orchestrator --test real_acp_smoke -- --nocapture
```

`SURGE_REAL_ACP_KIND` accepts `claude-code` / `codex` / `gemini-cli` / `custom`
(inferred from the binary name when omitted). `SURGE_REAL_ACP_ARGS` passes extra
launch args.

The smoke exercises the full launch path: **spawn → initialize handshake →
new_session → prompt** (the agent is told to immediately report a `done`
outcome and touch no files). A failure pinpoints which stage broke.

## Known quirks

- **Codex** uses an `acp` subcommand, not an `--acp` flag (direct-CLI model).
- **Gemini / Copilot** carry `--acp` inside the npx `default_args`; do not add
  it again when launching directly.
- Cross-runtime artifact uniformity (same flow producing format-equivalent
  `description.md` / `roadmap.toml` / `flow.toml` across runtimes) is verified
  by the artifact-contract golden tests; a live cross-agent golden compare is
  operator-pending where a second runtime is available.
- **Injected tools (`report_stage_outcome`, `request_human_input`) are not yet
  transported to real ACP agents.** `SessionConfig` declares them and the
  mock agent hard-codes the call; a real agent never receives the tool
  definition, so a stage on a real runtime reaches the prompt but cannot
  terminate through the injected tool. A real-Claude run hangs exactly as an
  Ollama one does. This is pre-existing and tracked separately from provider
  support; `ollama_acp_smoke` is deliberately transport-level because of it.
- **DeepSeek Harness (`dsh`)** ships only pre-release npm versions, so
  `RuntimeVersionPolicy` pins an exact build (`versions.toml`) rather than a
  floor. `dsh` has no separately-installed binary (`cli_binary = null`) —
  surge always launches and probes it via npx, never a same-named `dsh`
  binary that may not exist. `RegistryEntry::version_probe_args` carries the
  extra `npx` args (`-y @deepseek-ai/dsh`) needed to probe the actually-
  launched package's version rather than npx's own. `surge doctor agent
  dsh-acp --real` refuses loudly — both on a version mismatch *and* on "no
  data to compare" — instead of the warn-only path floor-style runtimes take
  (`is_exact_pin` in `crates/surge-cli/src/commands/doctor.rs` decides which
  path a runtime gets). `--handshake-only` stops that smoke after spawn + the
  ACP handshake, before sending a prompt — the version check and the
  handshake both need only the runtime installed, not its own model
  credentials, which is what `.github/workflows/dsh-canary.yml`'s hard gate
  relies on. Its ACP profile (`dsh --profile acp`) is a documented
  zero-option command, so every sandbox-matrix row stays declared-unverified
  — there is no CLI flag to verify a tier against.

