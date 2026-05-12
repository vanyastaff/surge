---
status: accepted
deciders: vanyastaff
date: 2026-05-11
supersedes: none
---

# ADR 0006 — ACP-only agent transport

## Context

By 2026 the [Agent Client Protocol][acp] registry catalogs ~33 coding agents that implement ACP, and the registry enforces a standardized `authMethods` exchange during the protocol handshake. Every commercial-subscription coding CLI surge targets is ACP-conformant or in adapter form: Claude Code, Codex CLI, Gemini CLI, Cursor CLI, GitHub Copilot CLI (public preview), JetBrains Junie, Augment, Kimi, OpenCode, Goose, and a long tail of registry entries beyond those.

Surge's surrounding invariants constrain the choice of agent transport:

- **Event-sourced run log with deterministic fold.** Replay must produce byte-identical state at every `seq`, across SDK and CLI updates.
- **Declared hooks** (`pre_tool_use`, `post_tool_use`, `on_outcome`, `on_error`) need tool-call boundaries as first-class events, not parsed sentinels.
- **Injected tools** (`report_stage_outcome`, `request_human_input`) must be exposed to every agent session through a uniform mechanism.
- **Sandbox elevation roundtrip.** When a runtime asks to elevate (write outside the workspace, escape sandbox, etc.) the engine writes `SandboxElevationRequested`, dispatches a Telegram card, waits for approval, and resumes the session through a protocol-level permission callback.
- **Multi-channel HumanGate approvals.** The same gate node must be servable from Telegram, desktop, email, Slack, or a webhook; the agent side of the conversation must be neutral to which channel carried the human's reply.

These invariants are not negotiable for v0.1. The transport mechanism must serve them, not weaken them.

## Decision

ACP is the **sole** agent-transport mechanism in surge. The `surge-acp` crate is the only agent-side adapter; no second backend is introduced for non-ACP agents.

## Rationale

Four forces point in the same direction, ranked in priority order:

1. **Parser-maintenance avoidance** (primary). Coding-agent CLIs ship frequent updates — sometimes weekly. Per-CLI stdout parsers (NDJSON formats, log prefixes, tool-call markers) would constitute a permanent treadmill of unrelated maintenance: every CLI minor bump risks silently breaking surge until a parser is patched. ACP fixes this surface as a versioned contract that absorbs CLI-internal changes upstream.
2. **Subscription-CLI coverage.** Every commercial-subscription coding CLI surge cares about supports ACP in 2026 — either natively (Goose, fast-agent, code-assistant) or through an adapter (Claude Code, Codex CLI, Pi). The subscription constraint on the user side — "use the Pro/Max/Team plan you already pay for, do not paste an API key" — is satisfied because subscription auth lives in the CLI's own config (`~/.claude/`, `~/.codex/`, etc.); ACP does not interfere with it. The non-ACP world is overwhelmingly API-key tooling, which contradicts that constraint.
3. **Structural fit with surge invariants.** Event-sourcing, hooks, injected tools, sandbox elevation, multi-channel approvals all map onto ACP primitives (typed JSON-RPC events, tool-call boundaries, permission callbacks, session resume). A non-ACP backend would either weaken these invariants or rebuild the same primitives on top of stdout — at which point it is ACP, badly.
4. **Auth-handshake standardization.** The [ACP registry][registry] mandates that conformant agents return a valid `authMethods` array during handshake. Surge selects subscription, OAuth, or API-key via the protocol, with no per-CLI config-file logic. The set of supported auth methods grows automatically as the registry grows.

## Alternatives Rejected

**Non-ACP raw headless CLI wrapping.** A `providers/<name>/{runner, commands, telemetryParser, auth, mcp_adapter}` plugin model — the shape CodeMachine-CLI uses — was considered. It is rejected because: (a) every CLI requires its own stdout parser maintained against frequent releases; (b) no bidirectional permission callback exists in headless mode, forcing preapprove-all flags like `--dangerously-skip-permissions` and breaking the sandbox-elevation invariant; (c) event-log determinism becomes CLI-version-dependent; (d) injected tools have no first-class transport.

**Direct LLM API calls bypassing CLIs.** Calling Anthropic, OpenAI, or Google APIs directly from surge eliminates the CLI layer entirely. Rejected because it forces a per-token billing model — the user must paste an API key — which contradicts the subscription constraint anchoring surge's user-facing value proposition.

## Consequences

- `surge-acp` remains the sole agent-side adapter crate. The 12-crate workspace structure described in `docs/ARCHITECTURE.md` and `.ai-factory/DESCRIPTION.md` does not gain a parallel `surge-agent-headless` peer.
- The **Sandbox delegation matrix** roadmap milestone widens as ACP coverage grows. Initially scoped to `{Claude Code, Codex CLI, Gemini CLI}`, it can extend to Cursor, Copilot, Junie, Augment, OpenCode, Goose, and further registry entries without architectural change.
- `surge init` PATH-scan should widen alongside the matrix to detect ACP-conformant CLIs the user has installed.
- New ACP-conformant agents that join the registry work with surge automatically — modulo capability cross-check (see below).

### Accepted costs and mitigations

Three concrete costs of ACP are known and acknowledged. Each is paired with a planned mitigation:

| Cost | Mitigation |
|---|---|
| `!Send` futures from the [`agent-client-protocol`][acp] SDK | The bridge runs on a dedicated OS thread with its own single-threaded Tokio runtime and `LocalSet`. Already implemented in `surge-acp`; communication with the engine is via typed `BridgeCommand` / `BridgeEvent` channels. |
| `unstable_session_usage` cargo feature exposes pre-1.0 protocol surfaces | Pin the SDK revision in workspace `Cargo.toml`, not track latest. Maintain CI golden-file tests against at least two real ACP agents (Claude Code and Codex CLI) plus the mock agent — already required by the Artifact-format milestone, verify it executes. |
| Adapter quality variance — native-ACP agents and adapter-wrapped agents (Claude, Codex, Pi) differ in feature parity, producing a three-layer debug surface (engine → ACP → adapter → agent) | Capability cross-check at handshake — surge declares the capabilities each profile needs; sessions that mismatch fail fast with a clear error. Provide a `--trace-acp` CLI flag that dumps the raw JSON-RPC stream when investigating bridge issues. |

## Out of Scope

Open-source CLIs that ship without ACP and have no path to subscription auth — Aider, Plandex, Continue, Crush, RA.Aid, Devon, and similar multi-provider API-key tooling — are out of scope. Supporting them would require both a non-ACP backend (rejected above) and a billing posture (API-key per-token) that contradicts the user-facing constraint anchoring surge's stance.

## Revisit conditions

This decision is reopened if any of the following becomes true:

- The ACP specification stalls or stagnates for more than 12 months without movement on authentication or capability discovery.
- A dominant commercial-subscription coding CLI emerges that refuses ACP integration for more than 12 months and gains adoption comparable to Claude Code or Cursor.
- Surge's primary user base shifts to API-key workflows, making the subscription constraint no longer load-bearing.
- A breaking change in the SDK between point releases destabilizes the bridge in a way the team cannot absorb through the mitigations listed above.

When any of these triggers, reopen the ADR with a follow-up that either widens the transport surface or supersedes this decision.

[acp]: https://agentclientprotocol.com
[registry]: https://github.com/agentclientprotocol/registry
