# Research

Updated: 2026-05-11 18:27
Status: active

## Active Summary (input for /aif-plan)
<!-- aif:active-summary:start -->
Topic: ACP-only stance as Surge's sole agent-transport mechanism

Goal: Lock in "ACP is the only way to communicate with agents" with an explicit, recorded rationale — replace the bare declaration in CLAUDE.md with an ADR that future contributors can challenge on its actual grounds, not on guesses.

Constraints:
- User wants pure subscription-based auth (Claude Pro/Max, ChatGPT Plus/Pro, Cursor Pro, etc.). No per-token API-key billing flows.
- Maintainability: user explicitly flagged "I don't want to chase parser updates for every agent" — agent CLIs release frequently, and a non-ACP fallback would force surge to track each one's stdout format.
- Surge invariants that must hold: append-only event-sourced run log with deterministic fold; declared hooks (`pre_tool_use`, `post_tool_use`, `on_outcome`, `on_error`); injected tools (`report_stage_outcome`, `request_human_input`); sandbox elevation roundtrip; multi-channel HumanGate approvals.

Decisions:
1. ACP-only confirmed. Non-ACP (raw headless CLI wrapping) explicitly rejected as a parallel backend.
2. Primary rationale, in priority order:
   a. Subscription-CLI coverage by ACP is effectively complete in 2026 — every commercial-subscription coding CLI surge cares about (Claude Code, Codex, Gemini, Cursor, Copilot, Junie, Augment, Kimi, OpenCode) is in the ACP registry, in adapter form, or in public preview.
   b. Maintenance: user's stated concern. CLI updates ship weekly; raw-stdout parsers would be permanent treadmill. ACP gives a versioned contract that absorbs CLI changes upstream.
   c. Structural fit with surge invariants: event log, hooks, injected tools, elevation roundtrip all map to ACP primitives. Non-ACP forces N parsers + reimplementation of bidirectional control via stdout sentinels.
   d. Auth handshake standardization: ACP registry requires agents to return valid `authMethods` — surge selects subscription/API-key via protocol, not per-CLI config-file logic.
3. The "long tail" of non-ACP CLIs (Aider, Plandex, Continue, Crush, RA.Aid, Devon, etc.) is **out of scope** — those are API-key-world tools that contradict the subscription constraint.

Accepted costs of ACP (documented as risk register, not blockers):
- `!Send` futures from `agent-client-protocol` SDK → dedicated OS thread + single-threaded Tokio + `LocalSet` (already implemented in `surge-acp`).
- `unstable_session_usage` cargo feature → protocol churn risk; pin SDK revision, don't track latest.
- Adapter quality variance (native vs adapter agents) → three-layer debug surface when something breaks.

Open questions:
- "Sandbox delegation matrix" milestone in `.ai-factory/ROADMAP.md` currently scopes to {Claude Code, Codex CLI, Gemini CLI}. Should it expand to current ACP top-tier subscription set: + Cursor CLI, Copilot CLI (public preview), Junie (when GA), Augment/Auggie, OpenCode, Goose?
- Should `surge init` PATH scan be extended beyond `claude` / `codex` / `gemini-cli` to also detect cursor, copilot, opencode, goose, augment binaries?
- ACP risk register: which mitigations land before v0.1?
  - Pin agent-client-protocol SDK to a specific rev.
  - Golden-file tests against Claude + Codex + mock agent in CI (already in "Artifact format" milestone — verify it runs).
  - `--trace-acp` flag to dump raw JSON-RPC for debugging.
  - Capability cross-check: surge declares expected capabilities per profile, fails fast if agent handshake doesn't advertise them.
- JetBrains Junie ACP support is "in progress" — is this a v0.1 blocker, post-v0.1 nice-to-have, or non-goal?
- Copilot CLI ACP is in public preview (Jan 2026 announcement) — wait for GA or include now with feature flag?
- OpenCode is interesting as a "meta-backend": user configures OpenCode with their own provider, surge talks to OpenCode via ACP, OpenCode talks to anything. Worth recommending in `surge init` as an optional universal gateway?

Success signals:
- ADR file at `docs/adr/<NNNN>-acp-only-transport.md` capturing the decision, primary rationale (with maintainability as headline), explicit non-goals (non-ACP CLI long tail), and the three accepted costs with their mitigations.
- "Sandbox delegation matrix" milestone in ROADMAP.md updated with the broader ACP-subscription-CLI set.
- `surge init` extended PATH scan list.
- Risk-register tickets opened (pin SDK rev, `--trace-acp`, capability cross-check).
- Golden-test parity in CI verified against at least 2 real ACP agents.

Next step: `/aif-plan` for the ADR + matrix expansion task; possibly `/aif-architecture` to mirror the ACP-only commitment into `ARCHITECTURE.md`.
<!-- aif:active-summary:end -->

## Sessions
<!-- aif:sessions:start -->
### 2026-05-11 18:27 — ACP-only confirmed; non-ACP rejected
What changed:
- Surveyed two reference projects (bytedance/deer-flow, moazbuilds/CodeMachine-CLI) and mapped their patterns against surge's existing architecture.
- Initially sketched a "trait AgentBackend with capability flags" proposal that would have admitted non-ACP fallback for non-ACP CLIs (Cursor, Auggie, OpenCode). Retracted after researching the ACP registry.
- Researched the actual 2026 ACP coverage: ~33 agents in the official registry (github.com/agentclientprotocol/registry), including every commercial-subscription CLI surge cares about. The non-ACP world is overwhelmingly API-key OSS tools (Aider, Plandex, Continue, etc.), which contradicts surge's subscription constraint.
- Clarified an earlier ambiguity: ACP and subscription auth are orthogonal axes. Subscription auth lives in the agent CLI's own config (`~/.claude/`, `~/.codex/`, etc.); ACP and headless are equivalent in this respect. ACP additionally standardizes the auth-method handshake.
- User reaffirmed ACP-only and added the primary maintenance argument: "I don't want to chase parser updates for every agent — they release frequently." This is now the headline rationale alongside structural fit and subscription coverage.

Key notes:
- ACP-only is not a side-effect of "we like protocols"; it derives from three orthogonal forces that all point the same way: (1) subscription coverage, (2) parser-maintenance avoidance, (3) structural fit with event-sourcing + hooks + elevation roundtrip + injected tools.
- The three real costs of ACP (!Send / unstable feature / adapter variance) are concrete and known; they need explicit mitigations in a risk register but they are not architectural objections.
- CodeMachine's `providers/<name>/{runner.ts, commands.ts, telemetryParser.ts, auth.ts, mcp/adapter.ts}` pattern is what surge would inherit by going non-ACP. ~5 files per agent × 7+ agents = permanent maintenance load, exactly what the user wants to avoid.
- Re-evaluation correction: in an earlier turn I implied Cursor/Auggie/OpenCode lacked ACP. They all support ACP as of 2026 (Cursor: cursor.com/docs/cli/acp; OpenCode: in registry; Augment: docs.augmentcode.com/cli/acp).

Links (paths):
- `CLAUDE.md` (project root) — currently states "ACP is the ONLY way" as bare declaration; needs ADR backing
- `.ai-factory/DESCRIPTION.md` — Core Features section references ACP bridge; aligned
- `.ai-factory/ROADMAP.md` — "Sandbox delegation matrix" milestone scopes only to {Claude, Codex, Gemini}; should be widened
- External: github.com/agentclientprotocol/registry, agentclientprotocol.com/get-started/agents, zed.dev/acp
<!-- aif:sessions:end -->
