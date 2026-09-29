# Comparison with Factory, Aperant and Agent OS — 2026-09-29

Status: decision note. Competitor facts come from their public pages fetched on
2026-09-29 and are quoted as claims, not verified behaviour. Surge facts point
at code in this tree. "Better" is claimed only where the difference can be
pointed at; where a competitor is ahead this page says so.

Sources: [Aperant repository](https://github.com/AndyMik90/Aperant),
[Factory documentation](https://docs.factory.ai/),
[Agent OS by Builder Methods](https://github.com/buildermethods/agent-os),
[awesome-agent-orchestrators](https://github.com/andyrewlee/awesome-agent-orchestrators).
"AgentLOS" did not resolve to a single product; the nearest matches are the
Agent OS projects, so they are what is compared here.

## What each one is

| | Shape | Agents it drives | Distribution |
|---|---|---|---|
| **Surge** | Event-sourced orchestrator: desktop app, CLI, daemon, MCP server | Any ACP agent: Claude Code, Codex, Gemini, Copilot, Cursor, OpenCode, Goose, DeepSeek Harness, Ollama | Local, Rust, one binary set |
| **Aperant** | Electron desktop app running up to 12 Claude Code terminals in worktrees | Claude only (Pro/Max subscription) | Local desktop, AGPL-3.0; 3.0 adds cloud |
| **Factory** | Managed platform: Droid CLI, app, web, mobile, headless "Droid Computers" | Multiple models behind Droid | Hosted/enterprise |
| **Agent OS** | Standards + spec-writing layer injected into an existing agent | Whatever agent you already use | Files in the repo |

## Where Surge is ahead, with the evidence

| Claim | Evidence in this tree |
|---|---|
| Plans are documents a human approves or sends back before any code runs | Bootstrap: description → roadmap → flow, each behind a gate with `approve` / `edit` / `reject` (`docs/bootstrap.md`) |
| The orchestrator sizes the plan instead of forcing a template | Roadmap stages are optional and free-form; the planner prompt says to choose the smallest structure and justify the stage count (`crates/surge-core/bundled/profiles/roadmap-planner-1.0.toml`, `docs/conventions/roadmap.md`) |
| Roadmap has stages, missions, milestones, tasks, priorities, dependencies and explicit parallel groups, all validated | `crates/surge-core/src/roadmap.rs` (`validate_ledger`, `ready_batches`) |
| "Done" is decided by a verifier that is not the implementer, and that difference is visible | `verified` is a separate field from `completed`; W4/W5/W6 flow-load warnings for unverified, same-runtime and end-only verification (`crates/surge-core/src/validation/`) |
| Every step runs a profile with its own role, icon, colour and reasoning floor | `Role.color` / `Role.min_effort`, floor enforced in `engine/stage/agent.rs` |
| A main agent can drive the whole system over MCP, with writes off by default | `surge mcp serve [--allow-write]`; `resolve` cannot approve bootstrap gates (`crates/surge-cli/src/commands/mcp_serve.rs`) |
| Work survives crashes, and any run can be replayed, forked or steered | Per-run SQLite event log, `surge-daemon::recovery`, `surge engine replay`, `fork`, `steer` |
| Not tied to one vendor | ACP-only transport ([ADR-0006](adr/0006-acp-only-transport.md)); cross-vendor verifier profile |
| Provider rate-limit windows park a run instead of stalling it | `engine/capacity.rs` |
| Reviewing an existing change, including AI-written code, is a first-class flow | `code-review` archetype |

## Where the others are ahead

- **Factory** has hosted cloud sessions synced across devices, web and mobile
  surfaces, headless machines, enterprise identity and governance. Surge is local
  only. That is a real gap for teams that want to start work from a phone.
- **Aperant** has a shipped desktop app with packaged Linux/Windows/macOS
  installers and a much larger user base (13.5k stars at the time of writing). It
  also advertises AI-assisted merge-conflict resolution; Surge's merge gate is
  aimed at PR auto-merge, not conflict resolution.
- **Agent OS** is far lighter to adopt: a folder of standards, no runtime.

## What this page does not show

No benchmark, user study or head-to-head run of these products exists in this
repository. Nothing here is evidence that Surge produces better code faster;
it is evidence about which controls exist. The honest way to close that is a
shared task suite run on each product, which has not been done.

## Modern-practice checklist

| Practice | Surge |
|---|---|
| Spec-first, human-approved plan | Yes (bootstrap gates) |
| Agents in isolated worktrees | Yes (`git2` worktree per task) |
| Independent verification | Yes, with cross-vendor option |
| MCP client (tools for agents) | Yes, supervised lifecycle ([ADR-0014](adr/0014-mcp-server-lifecycle.md)) |
| MCP server (agent drives the tool) | Yes, `surge mcp serve` |
| Agent Skills / plugin packs, hash-pinned | Yes (`surge skill`) |
| `AGENTS.md` / project context captured per run | Yes (`project.md`) |
| Cost and rate-limit control | Yes (budget freeze/resume, capacity parking) |
| Loop guard and output spill | Yes (`guard.rs`, `spill.rs`) |
| Replayable, auditable runs and a run report | Yes |
| Hosted / mobile / team features | No |
