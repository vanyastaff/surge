# Agent OS and coding-agent landscape

Status: research note, last checked 2026-07-07.

This page tracks adjacent products and patterns that matter for Surge. It is not
a support matrix; see [Agent runtimes](agent-runtimes.md) for Agent Client Protocol (ACP) runtime wiring.
The goal here is product direction: what each tool is trying to own, what Surge
can learn from it, and where Surge should deliberately be different.

## Strategic framing

The market is splitting into layers:

| Layer | Examples | What they own |
|---|---|---|
| Agent harness | Pi, Claude Code, Codex CLI, OpenCode | One agent loop, tools, context, model routing |
| Agent multiplexer | Herdr, tmux-style setups | Many live agent sessions, panes, state, reattach |
| Product coding agent | Devin, Factory Droid, Replit Agent, Copilot cloud agent | Ticket-to-code, PRs, app generation, managed workflow |
| Agent OS / gateway | OpenClaw, AgentOS papers, TopoClaw, AgentRM | Cross-app actions, memory, scheduling, permissions, skill routing |
| Surge target | Surge | Cross-runtime completion layer: task ledger, verification gates, event log, approvals, worktrees, runtime routing |

Surge should avoid becoming "one more coding agent". The stronger direction is
to be the local-first **completion layer** above many agents: a ledger that
can't lose work, verification that can't be gamed, and a replayable event log —
see [`product-strategy.md`](product-strategy.md) for the full decision note.

## Pi Coding Agent

Sources:

- https://pi.dev/
- https://github.com/earendil/ (linked from pi.dev; project ownership may move)

What it is:

Pi is a minimal coding-agent harness. It is intentionally not a sealed product.
The pitch is "adapt Pi to your workflows, not the other way around."

What it does:

- Provides an interactive TUI agent.
- Provides print / JSON mode for scripts.
- Provides RPC over stdin/stdout and an SDK for embedding.
- Loads `AGENTS.md`, `SYSTEM.md`, skills, prompt templates, and extensions.
- Stores sessions as trees, so users can rewind, branch, export, and share.
- Supports many providers: Anthropic, OpenAI, Google, Azure, Bedrock, Mistral,
  Groq, Cerebras, xAI, Hugging Face, Kimi, MiniMax, NVIDIA, OpenRouter, Ollama,
  and custom providers.
- Uses TypeScript extensions to add commands, tools, keyboard shortcuts, TUI
  changes, context filters, memory, compaction, and workflows.

Strong ideas to study:

- Minimal core plus extension surface.
- Session tree instead of linear chat log.
- `AGENTS.md` loaded from global, parent, and current directories.
- Skills and prompt templates as lightweight packageable context.
- JSON/print/RPC/SDK modes for automation and embedding.
- Self-customization: users can ask Pi to modify Pi extensions in place.

Protocol deep-dive (checked 2026-07-07):

- RPC mode (`pi --mode rpc`) is newline-delimited JSON, **not** JSON-RPC and
  not ACP. It is a much wider agent-operation surface than ACP: `steer` /
  `follow_up` (mid-turn redirection with configurable queue semantics),
  `fork` by any session entry, `get_tree`, `get_entries since=<cursor>`,
  `get_session_stats` (tokens/cost/context), model and thinking-level
  hot-swap, compaction control, host-initiated `bash`.
- Sessions are JSONL trees (`id`/`parentId` per entry) with **in-place
  branching**; abandoning a branch can inject a summary of it into the new
  branch (`branchWithSummary`) — a strong idea for fork ergonomics.
- No first-party ACP; a community adapter (`pi-acp`, listed in the Zed agent
  registry) proves Pi's RPC downcasts cleanly to ACP. First-party support is
  an open discussion with no maintainer commitment.
- Multi-agent is entirely extension-land (pi-subagents, pi-fleet, tmux
  wrappers): worktree isolation and parallel groups exist, but no event-log
  recovery, no lifecycle FSM, no durable run semantics anywhere in the
  ecosystem.

Weaknesses / gaps:

- No built-in MCP by default.
- No built-in subagents.
- No built-in plan mode.
- No built-in permission popups.
- No built-in background bash.
- Sandboxing, path protection, and approval gates are extension/package work.
- This is powerful but can become unsafe or inconsistent without policy.
- RPC protocol is single-vendor, unversioned, no capability negotiation;
  permission handling is an ad-hoc extension-UI bridge, not a first-class
  protocol method like ACP's `session/request_permission`.

Surge takeaways (decision: **no Pi-specific connector — reference, not
dependency**):

- Pi is a runtime/harness, not a full orchestrator; its multi-agent ecosystem
  is ad-hoc process supervision with no durability — Surge's differentiation
  holds.
- If Pi ships first-party ACP, it becomes one more runtime through the
  standard path. We do not build or depend on a Pi-RPC bridge.
- Implement Pi's best primitives natively in Surge: a steering queue
  (deliver operator messages at the next safe boundary, emulated over ACP),
  branch summaries on fork-from-here, and per-run token/cost introspection
  surfaced in the CLI/cockpit.
- Surge should copy the extensibility lesson, not the lack of policy: keep
  customizability, but bind it to typed flow nodes and auditable events.

## Herdr

Sources:

- https://herdr.dev/
- https://herdr.dev/docs/integrations/
- https://github.com/ogulcancelik/herdr

What it is:

Herdr is an agent multiplexer: tmux/Zellij-style terminal persistence with
agent-aware state. It is not a coding agent and not a workflow engine.

What it does:

- Runs agents in real PTY panes.
- Provides workspaces, tabs, panes, mouse support, detach, and reattach.
- Keeps pane processes alive after the terminal client exits.
- Works over normal SSH and native remote attach.
- Shows semantic state: `blocked`, `working`, `done`, `idle`.
- Provides a CLI and JSON socket API so tools and agents can create panes, read
  output, wait for state changes, and drive layouts.
- Supports integrations for Pi, Claude Code, Codex, Droid, Devin CLI, GitHub
  Copilot CLI, Kimi Code CLI, OpenCode, Kilo Code CLI, Hermes Agent, Qoder CLI,
  Cursor Agent CLI, MastraCode, and others.

Strong ideas to study:

- Agent state rollups across many live sessions.
- Real terminal panes instead of reconstructed chat views.
- Persistent PTY sessions with remote/mobile attach.
- Socket API for agent-driven orchestration.
- Status authority model: some integrations report lifecycle state, others only
  report native session identity for restore.
- "Operator console" UX: blocked sessions stand out immediately.

Protocol deep-dive (checked 2026-07-07):

- Socket API: newline-delimited JSON over a Unix socket (named pipe on
  Windows); the CLI mirrors it 1:1. Full surface: `agent.start/send/wait`,
  `pane.run/send_keys/read`, `events.subscribe` on
  `pane.agent_status_changed`, layout ops, even its own `worktree.*` methods.
- The agent state model is the catch: for Claude Code / Codex / Copilot the
  `blocked/working/done` signal comes **entirely from screen-manifest
  detection** (regex/pattern matching on the live TUI snapshot); installed
  hooks only report session identity for restore. Herdr's own docs admit
  session-only integrations "can miss permission approval results, escape
  interrupts, or other transitions". "Done" means "the TUI looks quiescent",
  not "the task succeeded".
- External tools *can* report authoritative state into Herdr via
  `pane.report_agent --source <id> --state <s>`, which disables its scraping
  for that pane.
- License is AGPL-3.0 + commercial dual; single primary author; fast-moving
  (breaking ID-format changes shipped inside a minor release).

Weaknesses / gaps:

- No workflow graph.
- No event-sourced run history.
- No native backlog intake.
- No policy engine for retries, approvals, or completion criteria.
- No artifact contract validation.
- State detection for the runtimes that matter is formalized screen scraping.

Surge takeaways (decision: **no Herdr bridge — build the cockpit natively**):

- Herdr is the strongest direct reference for multi-agent terminal
  *operations UX*: blocked-first triage ordering, state rollups, notification
  discipline (debounced "finished or input-needed" toasts), remote/mobile
  attach.
- Surge's state signal is strictly stronger than anything Herdr can detect:
  outcomes come from `report_stage_outcome` and ACP events, not from a screen.
  A cockpit rendered from the event log beats one reconstructed from a PTY —
  bridging would export our authoritative signal into a weaker medium and add
  a dependency on a fast-moving AGPL project.
- What Surge implements natively instead: `surge inbox` (Needs input /
  Working / Done triage), a run cockpit rendered from the event log, and
  remote attach through Surge's own daemon surfaces. Explicitly **not** a
  terminal multiplexer: Surge renders its own runs, it does not manage
  arbitrary PTYs.
- Durable run semantics stay the differentiation: graph node, attempt,
  approval, event, artifact, verification, outcome — a run is understandable
  without any screen.

## Factory Droid / Factory.ai

Sources:

- https://factory.ai/
- https://docs.factory.ai/

What it is:

Factory is an AI-native software development platform. Droid is its coding agent
surface. The broader pitch is a "Software Factory" that spans the SDLC.

What it does:

- Droid CLI and Factory app for interactive work.
- Droid Exec for headless CI/CD and automation.
- Droid Computers for persistent local or cloud machines.
- Automations, missions, automated code review, local code review, security
  review, incident response, automated QA, AutoWiki, Droid Control.
- Integrations with IDEs, Linear, Slack, MCP, skills, hooks, custom droids,
  plugins, mixed models, and BYOK.
- Enterprise deployment options: SaaS, hybrid, on-prem, air-gapped.

Strong ideas to study:

- They sell the whole SDLC, not just code generation.
- Model independence is a product feature.
- Sovereign deployment is a product feature.
- Agent readiness scoring: measure whether a repo is ready for autonomy.
- Droids/subagents as named specialized workers.
- Automations around triage, validation, release, monitoring, and documentation.

Weaknesses / gaps:

- Enterprise-heavy and likely high-touch.
- Hard to verify claims without being a customer.
- Potential lock-in around Factory's control plane.
- Broad SDLC coverage can hide unclear boundaries between agent, orchestrator,
  and policy engine.

Surge takeaways:

- Factory validates the "software factory" framing.
- Surge's local-first, Rust daemon, event log, and Telegram approvals are a
  differentiated alternative to a managed enterprise platform.

How Surge can be better:

- Be transparent and composable rather than opaque SaaS-first.
- Keep workflows as plain `flow.toml`.
- Make every run inspectable from event history and artifacts.
- Let teams use their preferred agents instead of one vendor's droid.

## Devin / Cognition

Sources:

- https://docs.devin.ai/
- https://www.cognition.ai/blog/introducing-devin

What it is:

Devin is a managed AI software engineer for engineering teams.

What it does:

- Writes, runs, and tests code.
- Handles Linear/Jira tickets, bug reports, feature work, app testing, PR review,
  codebase Q&A, unit tests, documentation, migrations, refactors, modernization,
  demos, integrations, and internal tools.
- Provides web/cloud, CLI, desktop, API, embedded IDE, shell, browser, and
  developer tools.
- Supports taking over Devin's work in the IDE.
- Has DeepWiki, knowledge onboarding, playbooks, automations, scheduled
  sessions, session insights, and integrations with Slack, Teams, GitHub,
  GitLab, Bitbucket, Linear, and Jira.

Strong ideas to study:

- Clear "AI teammate" packaging.
- Backlog carving: start tasks and return to draft PRs.
- Human takeover in the agent's IDE.
- Knowledge onboarding as a productized step.
- Practical guidance: tasks need explicit completion criteria and verification.

Weaknesses / gaps:

- Managed system with less local control.
- Success depends heavily on task scoping and verifiability.
- Hard tasks still need decomposition and senior review.
- Enterprise security and cost are external dependencies.

Surge takeaways:

- Devin proves that "backlog-to-draft-PR" is a valuable product surface.
- Surge should support the same workflow, but as a local-first orchestrator over
  multiple agents and worktrees.

How Surge can be better:

- Make task decomposition explicit in the flow graph.
- Keep local repository/worktree ownership visible.
- Require verifier nodes before presenting "done".
- Record why a task completed, not only what changed.

## OpenAI Codex

Sources:

- https://developers.openai.com/codex/
- https://platform.openai.com/docs/codex

What it is:

Codex is OpenAI's software engineering agent surface across CLI, cloud, IDE, and
web/product integrations.

What it does:

- Reads and edits code.
- Runs commands and tests.
- Can work in cloud/background sessions.
- Can integrate with GitHub and PR workflows.
- Provides a strong general-purpose coding model/agent experience.

Strong ideas to study:

- Cloud background task model.
- Reviewable diffs before merge.
- Integration between local CLI and managed cloud execution.
- Official model/runtime alignment.

Weaknesses / gaps:

- Not a full workflow graph engine by itself.
- Runtime behavior and cloud environment are vendor-controlled.
- For high-trust unattended work, teams still need policy, approvals, logging,
  and verification outside the model.

Surge takeaways:

- Codex is a runtime Surge should route to, not a thing Surge needs to replace.
- ACP support is already aligned with Surge's architecture direction.

How Surge can be better:

- Treat Codex as one worker among several.
- Persist every prompt, approval, tool-facing decision, artifact, and verifier
  outcome in Surge-owned storage.

## Claude Code

Sources:

- https://docs.anthropic.com/en/docs/claude-code/overview

What it is:

Claude Code is an agentic coding tool for terminal, IDE, desktop, web, browser,
Slack, CI/CD, and custom agent workflows.

What it does:

- Reads codebases, edits files, runs commands, and uses development tools.
- Supports terminal, VS Code, JetBrains, desktop app, web, browser/computer use,
  Slack, GitHub/GitLab CI/CD, routines, scheduled tasks, remote control, and
  mobile/web handoff.
- Supports `CLAUDE.md`, auto memory, MCP, skills, hooks, background agents,
  multi-agent work, and Agent SDK.

Strong ideas to study:

- Excellent terminal-first UX.
- `CLAUDE.md` as practical repo-local context.
- Permission modes and hooks.
- Background agents and scheduled tasks.
- Remote control/channel integrations.
- Composable CLI.

Weaknesses / gaps:

- Shell access creates real prompt-injection and command-risk exposure.
- Local session state is not the same as a durable workflow event log.
- Multi-agent work can obscure accountability unless wrapped with policy.

Surge takeaways:

- Claude Code is the best current local agent runtime reference.
- Surge should use Claude via ACP where possible and avoid depending on
  Claude-specific prompt surfaces for core semantics.

How Surge can be better:

- Put permission decisions in a Surge approval ledger.
- Add deterministic gates around commands, artifacts, and output validation.
- Use worktree isolation per run/node.

## GitHub Copilot cloud agent and Copilot CLI

Sources:

- https://docs.github.com/en/copilot/concepts/agents/cloud-agent/about-cloud-agent

What it is:

Copilot cloud agent is GitHub-native background coding. Copilot CLI is the local
agentic command-line companion.

What it does:

- Cloud agent can research a repository, plan, make changes on a branch, and
  optionally open a PR.
- Can start from GitHub.com, issues, PR comments, VS Code, GitHub Mobile,
  Slack, Teams, Linear, Jira, Azure Boards, Raycast, API, GitHub CLI, and MCP.
- Uses an ephemeral GitHub Actions-powered development environment.
- Provides org metrics around PR outcomes, time to merge, and usage.

Strong ideas to study:

- Native issue-to-branch-to-PR flow.
- Transparent commits/logs in GitHub.
- Automation triggers on schedule or repository events.
- Enterprise metrics tied to PR lifecycle.
- Tight permission model around GitHub identity.

Weaknesses / gaps:

- GitHub-centric.
- Cloud agent is distinct from local IDE agent mode.
- Session/task limits and repo/branch/PR constraints apply.
- Less useful for non-GitHub or local-only workflows.

Surge takeaways:

- GitHub is the canonical integration point for many engineering teams.
- Surge should model task source, branch, PR, and review lifecycle explicitly.

How Surge can be better:

- Support GitHub without becoming GitHub-only.
- Keep Linear/GitHub/manual/Telegram intake behind the same run model.
- Allow local-first execution while still reflecting state back to GitHub.

## OpenHands

Sources:

- https://docs.openhands.dev/
- https://arxiv.org/abs/2407.16741

What it is:

OpenHands is an open-source/source-available platform and SDK for software
development agents.

What it does:

- Provides Agent Canvas, cloud, enterprise, CLI, local GUI, and Python SDK.
- Agents can write code, use command line, browse the web, and run in sandboxed
  environments.
- Supports GitHub/GitLab/Bitbucket, Slack, Jira, Linear, multi-user, RBAC,
  usage reporting, budgeting, hooks, MCP, skills, plugins, and automations.

Strong ideas to study:

- Open platform for custom agent development.
- SDK-first architecture.
- Local-to-cloud execution portability.
- Sandboxed execution as a built-in concern.
- Community benchmark/evaluation culture.

Weaknesses / gaps:

- Running and operating the stack is heavier than a single CLI.
- Enterprise features are not all MIT.
- More framework than opinionated AFK workflow product.

Surge takeaways:

- OpenHands is a reference for agent SDK and sandbox architecture.
- Surge can stay narrower: workflow execution and approvals over multiple
  external agent runtimes.

How Surge can be better:

- Be easier to reason about for a single repo/run.
- Keep flow files and event logs small, explicit, and local.
- Avoid requiring users to adopt a full agent application platform.

## Replit Agent

Sources:

- https://docs.replit.com/references/agent/overview

What it is:

Replit Agent turns natural-language ideas into apps, designs, slides,
dashboards, files, and deployed artifacts inside Replit.

What it does:

- Builds web apps, mobile apps, dashboards, AI tools, visual designs, prototypes,
  slide decks, videos, CSV/PDF/PowerPoint/Markdown files.
- Writes code, sets up infrastructure, tests work, and helps publish.
- Has Plan mode, Lite/Economy/Power modes, Design Canvas, multi-artifact
  projects, app testing, checkpoints, rollback, skills, tasks, connectors, MCP,
  databases, auth, payments, deployment, monitoring, and collaboration.

Strong ideas to study:

- Non-developer-friendly creation flow.
- Plan mode before code/data changes.
- Checkpoints and rollback are visible product features.
- One project can contain many artifacts sharing a backend.
- Design Canvas bridges visual edits and generated code.

Weaknesses / gaps:

- Best for Replit-hosted projects and prototypes.
- Less natural for mature local monorepos.
- Autonomy around app data requires strong safety controls.
- Credit/cost model can shape workflow.

Surge takeaways:

- Plan/approve/build/refine/publish is a useful user loop.
- Rollback/checkpoints should be first-class in any AFK agent workflow.

How Surge can be better:

- Keep mature codebase workflows as first-class, not only app generation.
- Use git worktrees and commits as checkpoints.
- Require explicit approvals before destructive or production-facing actions.

## Google Jules and Antigravity

Sources:

- https://jules.google.com/
- https://antigravity.google/

What they are:

Jules is Google's asynchronous coding agent around GitHub/Gemini. Antigravity is
Google's agent-first development environment/IDE.

What they do:

- Jules focuses on delegated coding tasks, GitHub issues, code changes, and
  async execution.
- Antigravity focuses on an IDE plus manager view for orchestrating agents, with
  access to editor, terminal, and browser.
- Public reporting emphasizes plans, screenshots, browser recordings, and other
  artifacts for trust.

Strong ideas to study:

- Agent manager view as a first-class UI.
- Visual artifacts for trust and review.
- Browser recordings/screenshots as validation evidence.
- Async delegated tasks as a normal dev workflow.

Weaknesses / gaps:

- Product shape has changed quickly.
- Strong Google/Gemini ecosystem dependency.
- Need to evaluate governance, deployment, and integrations case by case.

Surge takeaways:

- Visual/verifiable artifacts are important.
- Surge should collect evidence, not just final text.

How Surge can be better:

- Store validation evidence in the event log/artifact set.
- Keep evidence capture runtime-agnostic: tests, logs, screenshots, generated
  reports, and PR links should all fit the same model.

## Cursor

Sources:

- https://cursor.com/
- https://docs.cursor.com/

What it is:

Cursor is an AI-first editor/IDE with codebase understanding and agentic editing.

What it does:

- Provides interactive IDE-based code generation and editing.
- Indexes the codebase for chat and codebase Q&A.
- Supports multi-file changes, agents, rules, MCP, and CLI surfaces.

Strong ideas to study:

- Fast interactive developer UX.
- Codebase indexing and references in everyday editing.
- Low-friction adoption because it replaces a familiar editor.

Weaknesses / gaps:

- More developer cockpit than AFK workflow orchestrator.
- Work can remain local/session-bound unless pushed into PR/event workflow.
- IDE-centric model is less natural for daemon-driven runs.

Surge takeaways:

- Cursor is not the same layer as Surge.
- Surge should support editor handoff/review, but own run state elsewhere.

How Surge can be better:

- Make agent work visible after the editor closes.
- Keep task state, approvals, and verification outside a single IDE session.

## BridgeMind (BridgeSpace / BridgeSwarm / BridgeMCP)

Sources (checked 2026-07-07):

- https://www.bridgemind.ai/bridgeswarm
- https://www.bridgemind.ai/products/bridgespace
- https://docs.bridgemind.ai/docs/mcp
- https://www.bridgemind.ai/changelog

What it is:

Commercial "vibe coding" platform (closed-source SaaS, subscription + credits,
audience-first go-to-market: large YouTube/X following, 30% recurring
affiliate program). Solo founder; pivoted from a 2024 prompt-engineering
education site. Products are young (BridgeSwarm launched ~March 2026). Real
shipped software with a credible engineering changelog, but small
(~$200K ARR per an affiliated case study) and heavily self-amplified.

What it does:

- **BridgeSpace** — desktop "Agentic Development Environment" (Tauri 2 + Rust):
  up to 16 terminal panes, live mission tree, command bar with @-targeting of
  any running agent, BridgeBoard kanban where dragging a card dispatches an
  agent.
- **BridgeSwarm** — the multi-agent layer inside BridgeSpace: one prompt → a
  "mission" → fixed roles (Coordinator plans, Builders write, Scout explores,
  Reviewer gates merges) communicating through a shared mailbox. Concurrency
  via **file-level ownership** (each task exclusively owns files it touches;
  shared dependencies auto-sequenced), not worktree isolation. Drives vendor
  CLIs (Claude Code, Codex, Gemini CLI, OpenCode, Cursor, Grok) in PTY panes
  with injected hooks and output-text heuristics.
- **BridgeMCP** — hosted MCP server: a task tracker + agent-config CRUD. The
  interesting bit is `taskKnowledge`: up to 50k chars of curated context
  attached to each task, delivered to whichever agent (in whichever tool)
  picks the task up.
- **BridgeMemory** — repo-resident, git-committable markdown memory in
  `.bridgememory/`, linked as a graph, shared across heterogeneous agent CLIs
  over MCP.
- **BridgeCode** — their own CLI coding agent: announced, waitlisted, never
  launched, silently removed ("Removed the discontinued BridgeCode agent
  integration", changelog June 2026).

Strong ideas to study:

- Git-committed project memory that every agent reads and writes.
- Curated per-task context packets (`taskKnowledge`) instead of letting each
  agent rediscover context.
- @-addressability of any live agent from one command bar; live mission tree.
- File-ownership concurrency as a cheap complement to worktrees for
  dependency-disjoint parallel work inside one checkout.

Weaknesses / gaps:

- PTY-level supervision of vendor CLIs: activity/failure detection parses
  terminal output; their changelog documents the resulting bug class (resume
  misread as failure because the transcript contained "command not found").
  A concrete validation of ADR-0006 by counter-example.
- No spec formalism, no dependency graph beyond opaque Coordinator planning,
  no declarative workflow definition, no event-sourced recovery.
- Hosted-only MCP despite "runs locally" marketing; closed source; targets
  non-engineer "vibe coders".
- Tried to build their own coding agent and gave up — arrived at Surge's
  "orchestrate other vendors' agents" bet the hard way.

Surge takeaways:

- Steal the memory ideas natively: `.surge/memory/` (git-committed,
  provenance-tracked, audited for staleness) and curated context bindings per
  ledger task.
- Steal the interaction ideas natively: @-addressing live nodes, a live
  mission/run tree in the cockpit.
- Differentiate on everything structural: typed flow graphs vs. opaque
  Coordinator prompts, event-sourced recovery vs. heuristic session restore,
  ACP vs. PTY scraping, local-first open source vs. hosted subscription.

## 2026 convergence: first-party manager views and the orchestrator graveyard

Between October 2025 and June 2026 every major vendor shipped a first-party
multi-agent supervision surface:

| Vendor | Surface | Core model |
|---|---|---|
| Anthropic | Agent View (`claude agents`) | One screen; sessions grouped **Needs input / Working / Completed**; peek + reply in place |
| Google | Antigravity Agent Manager → 2.0 | Inbox + workspaces; artifact-based review (plans, screenshots, recordings); comment-on-artifact steers without stopping |
| OpenAI | Codex app | Parallel threads (local / worktree / cloud); mid-turn steering (Enter steers, Tab queues) — local only |
| Cursor | Agents Window (3.0+) | Agent tabs/grid; `/best-of-n` across models in worktrees; one-click pull-branch-to-foreground |
| Cognition | Devin Desktop | Kanban command center; **ships with ACP**, runs Codex / Claude Agent / OpenCode |

What this settles:

- The winning interaction patterns are now known, ranked by convergence:
  (1) status-triaged inbox of blocked runs; (2) worktree-per-run with a
  one-gesture promote-to-foreground; (3) artifact-level batch review instead
  of tool-call streams (approval fatigue is a documented failure mode);
  (4) steer-without-stopping; (5) thread-per-agent in a messaging surface
  with remote approvals (Surge's Telegram cards sit in this pattern).
- Every first-party view supervises **only its own agents**. Devin Desktop
  crossing vendors *via ACP* is both validation of ADR-0006 and the clearest
  competitive signal: cross-vendor, local-first, audit-grade supervision is
  the open slot.
- Honest concurrency ceilings from practitioners: 2–3 agents unstructured,
  3–5 with worktrees + test gates, 5–10 only with heavy up-front specs. The
  limit is human attention — which ties fleet UX directly to the completion
  machinery (specs/ledger) rather than to raw pane count.

The graveyard (early 2026) and its lessons:

| Tool | Fate | Lesson |
|---|---|---|
| Omnara (mobile command center) | archived; wrapping Claude Code CLI "unfeasible to maintain" | protocol-level integration or death |
| Terragon (cloud task list → PRs) | shut down; no traction | first-party vendors absorb thin orchestration |
| vibe-kanban | folded at 27k GitHub stars | kanban-of-tasks alone is not a product |
| BridgeCode | never launched, silently removed | building one more coding agent is a losing bet |

Full synthesis and what Surge builds in response: [`product-strategy.md`](product-strategy.md).

## Agent OS, Claude OS, and OS-style agent systems

Sources:

- AgentOS paper: https://arxiv.org/abs/2603.08938
- TopoClaw paper: https://arxiv.org/abs/2605.15556
- AgentRM paper: https://arxiv.org/abs/2603.13110
- OpenClaw overview/reporting: https://www.techradar.com/pro/what-is-openclaw

What this category is:

"Agent OS" is an emerging category, not one stable product. It describes a
kernel-like layer for agent lifecycle, memory, scheduling, permissions, skill
routing, cross-app execution, cross-device execution, and human accountability.

"Claude OS" appears mostly as informal shorthand around Claude Code plus
extensions/workflows. I did not find one canonical, official product named
Claude OS. Treat it as a meme/category unless a concrete project URL is known.

Important patterns:

- Agent kernel: intent parsing, decomposition, scheduling, execution.
- Skills-as-modules: apps/tools become composable capabilities.
- Context/memory manager: compaction, hibernation, retrieval, persistence.
- Permission/resource manager: access control, rate-limit-aware admission,
  zombie reaping, isolation, budget.
- Cross-device/cross-user topology: actions routed to the right device or human
  context with provenance and role-aware permissions.
- Gateway model: messages from Slack/Telegram/WhatsApp/webhooks route into local
  agents and tools.

Strong ideas to study:

- OS metaphors are useful when agents become long-running processes.
- Scheduling and resource management are real product problems.
- Context lifecycle is a first-class subsystem, not a prompt detail.
- Permissions must be attached to identity, context, tool, and action.
- Cross-channel UX matters: phone, chat, desktop, web, terminal.

Weaknesses / gaps:

- Many "Agent OS" projects are conceptual or early.
- The OS metaphor can become vague marketing.
- Broad computer-use agents expand security blast radius.
- Skill marketplaces can create supply-chain risk.
- Multi-agent decomposition can hide harmful or unintended outcomes from the
  model and the user.

Surge takeaways:

- Surge already has several OS-like pieces: daemon, event log, workflow graph,
  approvals, sandbox profiles, runtime registry, and Telegram cockpit.
- The better framing may be "local-first agent workflow OS for software
  development", but the implementation should stay concrete.

How Surge can be better:

- Use OS ideas only where they map to real mechanisms:
  - scheduler: run queue, concurrency, retries, budgets;
  - process: flow node attempt;
  - filesystem boundary: worktree/sandbox;
  - kernel log: event store;
  - permissions: approval cards and policy;
  - signals: cancel, pause, resume, timeout;
  - device/channel: CLI, daemon socket, Telegram, desktop UI.
- Avoid a generic personal-agent OS until the software-development workflow is
  excellent.

## Watchlist

These appeared as Herdr-supported or adjacent agents/tools. They are worth a
later focused pass before making integration decisions:

| Tool | Why watch |
|---|---|
| OpenCode | Open coding agent with plugin/state integration potential |
| Amp | Agentic coding tool; appears in Herdr support list |
| Hermes Agent | Reports lifecycle, tool, approval state, and session id to Herdr |
| Kilo Code CLI | Agent CLI with plugin-based Herdr integration |
| Qoder CLI | Agent CLI with session restore integration |
| Kiro CLI | Detected by Herdr, but blocked-state support appears incomplete |
| Kimi Code CLI | Native lifecycle + session identity in Herdr |
| Grok CLI | Appears in Herdr support list |
| Antigravity CLI | Appears in Herdr support list |
| MastraCode | Reports lifecycle state and thread identity to Herdr |

## Product direction for Surge

> Superseded in place: the full strategy — positioning, evidence, pillars,
> sequencing, metrics — now lives in [`product-strategy.md`](product-strategy.md).
> This section keeps the landscape-level summary.

Positioning:

**Surge is the cross-runtime completion layer.** Agents solve tasks; Surge
finishes projects. It does not try to beat Claude/Codex/Pi/Devin at being an
individual agent, and it does not multiplex their terminals — it owns the
ledger, the verification gates, and the replayable event log above all of them.

What Surge owns:

- A typed **task ledger** with dependencies, discovered-work capture, and
  context-budgeted task sizing (the successor of prose `roadmap.md`).
- **Ungameable verification**: verifier nodes as the sole write path to
  `done`, sealed sandbox intent, evidence compiled into a Run Report.
- `flow.toml` as the workflow contract; durable run/event history; replay and
  fork-from-here.
- Worktree lifecycle and isolation, with promote-to-foreground.
- Agent runtime routing over ACP — and only ACP.
- Fleet interaction rendered from the event log: blocked-first inbox,
  steering queue, native cockpit, Telegram digest.
- Accumulating, git-committed project memory with a staleness audit.
- Tracker intake and state reflection; recovery after restarts; policy around
  budget, sandbox, trust, and destructive actions.

What Surge deliberately does not build (see the graveyard above):

- Bridges/adapters to third-party multiplexers or agent-specific RPC surfaces
  (Herdr, Pi RPC) — their best ideas are implemented natively instead.
- PTY management or screen-based state detection of any kind.
- A proprietary coding agent or model.
- Model-specific prompting tricks as core semantics.
- A generic personal-assistant OS before the coding workflow is excellent.
- App-generation UX for non-developers unless it grows naturally from flows.

Differentiation sentence:

Managed agents and first-party manager views try to complete a task inside one
vendor's walls. Surge is the local-first layer that makes fleets of agents from
any vendor finish whole projects — with a ledger that can't lose work, a
verifier that can't be gamed, and an event log that can replay how it happened.

