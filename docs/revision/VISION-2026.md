# Surge · Vision 2026

> Open agent-agnostic, source-agnostic meta-orchestrator for autonomous AI coding.

## Where we are in the industry

April–May 2026 saw three vendors ship production-grade harness/orchestration products in the same month:

- **OpenAI Symphony** — open-source spec, ties Codex agents to Linear tickets via Codex App Server, runs in OpenAI's cloud devboxes.
- **Anthropic Claude Managed Agents** — fully cloud-hosted agent harness with built-in memory, multi-agent coordination, $0.08/hr pricing.
- **LangChain Deep Agents v0.5** — open-source Python/TS framework with async sub-agents, Agent Protocol, four-pillar architecture.

Plus the **OpenAI Agents SDK next evolution** delivered native sandbox execution across seven cloud providers (E2B, Modal, Daytona, Cloudflare, Vercel, etc.). Martin Fowler formalized the conceptual base: **Agent = Model + Harness**, with **guides** (predfeedback) and **sensors** (postfeedback), computational vs inferential, harness templates as the unit of reuse.

The pattern is clear. Coding agents themselves moved into clouds (Claude Code Cloud, Codex App Server, Copilot Cloud). The new battlefield is the layer above them — orchestration, intake, governance, audit, multi-agent coordination.

## Where Surge stands

Surge's differential position is `pure meta-orchestrator`:

- **Agent-agnostic** — speaks ACP, can drive any conformant agent (Claude Code, Codex, Copilot Agent, Aider when it ships ACP, etc.). Symphony is Codex-only; Claude Managed Agents is Anthropic-only; Deep Agents is LangChain-flavoured.
- **Source-agnostic** — ingests work from any tracker (Linear, GitHub, future Discord/Jira/Slack/Notion). Symphony is Linear-only.
- **Sandbox-delegated** — never reimplements isolation. Each agent has its own native sandbox (Codex CLI sandbox modes, Claude Code Skills isolation, etc.). Surge configures it via ACP, the agent enforces it. No Landlock/sandbox-exec/AppContainer code in our tree.
- **Local-first** — long-running daemon on user's machine, multi-channel notifications (Telegram, desktop, email). Cloud deployments possible later but not the primary form factor.
- **Mobile-first approval UX** — primary control surface is the user's phone via Telegram bot with inline keyboards. The vibe-flow vision: `describe → walk away → return to a PR`.
- **Event-sourced and replayable** — append-only event log, time-travel debugging, fork-from-here. Symphony stores state in Linear; Claude Managed Agents owns it server-side; we own it locally and openly.

## What we're building

Three follow-up RFCs extend the vibe-flow core (RFC-0001 through RFC-0008) to deliver the meta-orchestrator positioning. They depend on existing engine M-series (M1–M7 shipped) but each is independently shippable.

| RFC | Title | Status | Depends on |
|-----|-------|--------|-----------|
| RFC-0010 | Issue-tracker integration (Linear + GitHub Issues) | drafted | RFC-0001..0008, M1–M7 |
| RFC-0011 | Async subagents (Deep Agents v0.5 inspired) | planned | RFC-0010, M7 daemon |
| RFC-0012 | Harness templates (Fowler-inspired guides + sensors) | planned | RFC-0010, RFC-0004 |

These are **the ladder**, in order. RFC-0010 lands first because it does not require new fundamentals — it sits on top of existing `surge-spec`, `AgentPool`, FSM, daemon. It also delivers the most visible developer-facing value: tickets coming from Linear/GitHub turn autonomously into PRs, with the user only tapping "Start" / "Approve" in Telegram.

## What we explicitly do not build

To prevent scope creep and to keep our positioning sharp:

- **Sandbox abstraction crate / OS-level isolation.** Modern coding agents already isolate themselves (Codex CLI, Claude Code, Cursor). Building a parallel layer would be reinventing the wheel and produce "cloud over cloud" stack. RFC-0006's Tier 1+2 (MCP filtering, path checking) stays as basic guardrail; Tier 3+4 (Landlock, sandbox-exec, AppContainer, network isolation) is **deprecated** and will be removed in a future RFC-0006 refactor.
- **Cloud-hosted Surge.** Surge is local-first by design. The agent runs in its own cloud sandbox; Surge stays on the user's machine.
- **Web app / Telegram WebApp inside the bot.** Reserved for v2+ when there is real user demand.
- **Multi-user / team collaboration** on same run. Single-user single-machine. Future RFC.
- **Cross-tracker sync** (Linear ↔ GitHub mirroring). Out of scope.
- **CI/CD bot replacement.** We coordinate coding agents that produce PRs; we don't replace the merge-bot, deploy-bot, etc.

## Architectural axes

Two orthogonal axes structure the system:

**Input axis — task sources** (`surge-intake` crate, RFC-0010+):
- `LinearTaskSource`, `GitHubIssuesTaskSource` (RFC-0010)
- `DiscordTaskSource`, `JiraTaskSource`, `SlackTaskSource`, ... (future)
- CLI `vibe run`, Telegram `/run` (existing in v0.1, optionally wrapped as `TaskSource` later)

**Output / approval axis — notification channels** (`surge-notify` crate, M6+):
- `TelegramChannel` — primary cockpit-inbox; sees all inbound from all sources, all approvals, all status
- `DesktopChannel` — parallel local notifications
- `EmailChannel`, `SlackChannel`, `WebhookChannel` (M6 existing)

These axes do not mix. A `TaskSource` is "where work comes from"; a notification channel is "how Surge talks to the user about it". Linear and GitHub Issues are sources, not channels. Telegram is a channel, not a source — even though Telegram `/run` lets you start ad-hoc runs, that's a small CLI-equivalent path, not a peer of tracker integration.

## Connection to vibe-flow

This document does not replace `01-VISION.md` (the original vibe-flow vision). It extends it with the 2026 positioning and the post-RFC-0008 roadmap. The five core principles of vibe-flow remain unchanged:

1. Engine is dumb, agents are smart.
2. Adaptive complexity (Flow Generator).
3. Sandbox-first autonomy (now meaning: delegate sandbox to agents, do not reimplement).
4. Event-sourced, replayable.
5. Open source, MIT/Apache-2.0, no telemetry.

The codename "vibe-flow" remains internal to specs; production code uses `surge-*` prefixes.
