# Comparison with Factory, Aperant, Agentlas and the field — 2026-09-29

Status: decision note, second revision. The first revision compared feature
lists from landing pages. This one is built from Aperant's source code
(shallow-cloned and read on 2026-09-29), Factory's documentation and
independent reviews, the Agentlas repository, and 2026 articles and papers on
agentic-development practice. Claims are tagged **[code]** (read in source),
**[docs]** (vendor documentation), **[vendor]** (vendor marketing or
self-reported numbers) or **[review]** (independent write-up). Surge facts
point at this tree.

"AgentLOS" in the request is Agentlas (`agentlas-ai/agentlas-desktop`); an
earlier audit of it lives in [`vibe-coding-harness.md`](vibe-coding-harness.md).

What this page can and cannot show: **no shared benchmark of these products
exists**, and Factory's published numbers are self-run (below). Everything here
is a comparison of design and controls, sourced; it is not a claim about
generated-code quality.

## Aperant (AGPL-3.0, Electron/TypeScript, ~13.5k stars) — read from source

| Area | What the code does | vs. Surge |
|---|---|---|
| Process model | Vercel AI SDK agent loop in worker threads (`ai/agent/worker-bridge.ts`); state in `implementation_plan.json` and spec dirs; **no event log** | Surge drives external agents over ACP and keeps a per-run event log; crash recovery and replay come from it |
| Planning | Complexity assessor → gatherer → researcher → writer → critic → planner (`orchestration/spec-orchestrator.ts`). **No plan-approval gate**: tasks run on autonomously | Surge stops at description, roadmap and flow with approve / edit / reject |
| QA | Reviewer + fixer loop, up to 50 iterations (`orchestration/qa-loop.ts`). Same model and provider for both; the reviewer has the same write tools and **records its own verdict** into the plan JSON; nothing deterministic gates it | Verifier is a separate profile with a sealed `verified` ledger effect; W4/W5/W6 flag unverified, same-runtime and end-only verification at flow load; cross-vendor verifier supported |
| Merge | Deterministic semantic strategies first (imports, hooks, appends), single-turn AI only for ambiguous regions (`ai/merge/`); issue #1343: merge reverted changes on main | Surge has PR auto-merge behind a gate, **no semantic conflict resolver** — a real gap |
| Memory | SQLite, hybrid BM25 + dense + graph retrieval, code graph via tree-sitter (`ai/memory/`); external Graphiti/LadybugDB server is a recurring pain (#1775, #1328) | Surge memory is `add`/`search` with provenance; retrieval is simpler |
| Isolation/security | Worktree per task; bash **denylist** (`ai/security/denylist.ts`), no OS sandbox. Bugs: agent commits to main (#1444), `git clean -fd` deleted untracked files (#1477), orphaned worktrees (#1103) | Surge delegates to each runtime's native sandbox ([`sandbox-matrix.md`](sandbox-matrix.md)) |
| Providers | Own model-API loop over 11 providers; **not agent-agnostic** | ACP-only, any agent |
| Open issues | About a third are auth/login (#1909, #1798, #1768…), then startup errors, MCP setup, Windows/WSL, roadmap generation | — |
| UX Surge lacks | Multi-project kanban with drag order and queue, up to 12 terminals with task context, codebase Insights chat, changelog generation, issue import + PR review UI, screenshot/image attach | Surge has inbox, fleet, roadmap, missions, flow, memory screens; no kanban queue, no Insights chat, no changelog |

## Factory (proprietary, hosted control plane)

- **Missions [docs]** ([overview](https://docs.factory.ai/missions/overview),
  [planning](https://docs.factory.ai/missions/planning),
  [architecture](https://factory.com/news/missions-architecture)): orchestrator
  asks questions, writes a *validation contract*, splits work into milestones
  and features, each feature to a fresh worker session; at each milestone
  *scrutiny* validators review code and *user-testing* validators **drive the
  running app** against the contract (`agent-browser`, `tuistory`); failures
  become fix features. Docs describe no human approval beyond the planning
  conversation.
- **Benchmarks [vendor]:** Terminal-Bench 58.75% (Sep 2025), 63.1% (Dec 2025),
  run by Factory with its own harness; a comparison guide puts harness choice
  alone at 10–20 points. No independent replication found; the board has since
  moved to 4.0 ([docs](https://docs.factory.ai/benchmarks/terminal-bench),
  [review](https://rywalker.com/research/factory-ai)).
- **Complaints [review]:** unpredictable token cost and unpublished quotas;
  context-window opacity; a review bottleneck; dependence on Factory's relay
  and dashboards ([Continuum](https://continuumcode.ai/guides/factory-ai-review/),
  [lowcode](https://www.lowcode.agency/blog/claude-code-vs-factory-ai)). One
  favourable review exists ([Every](https://every.to/vibe-check/vibe-check-i-canceled-two-ai-max-plans-for-factory-s-coding-agent-droid)).
- **Enterprise-only [docs]:** SSO/SAML/SCIM, audit logs, ZDR, on-prem
  ([pricing](https://factory.com/pricing)).
- **Ahead of Surge:** persistent remote machines steerable from web/mobile;
  agent-driven QA of the running app as a first-class validator; milestone-level
  scrutiny + user-testing pair; polished review UI; enterprise governance.

## Agentlas Desktop (Apache-2.0, Electron, ~6 stars)

Local control room for agent teams (Claude, Codex, Gemini, Grok, Ollama, BYOK),
goal decomposition with autonomous replanning, MCP, browser automation, a mobile
bridge, signed releases. Verification/QA is thinly documented and the changelog
shows frequent breaking changes. Not a peer on verification, replay or plan
approval; its browser automation and mobile bridge are the parts Surge lacks.

## The rest of the field (2026 articles and comparisons)

Spec tools with no runtime — Kiro (requirements/design/tasks with approval;
reviewers report overhead on small tasks) and GitHub Spec Kit (markdown
artifacts, "sprawl"). Dispatchers with no gate or verifier — Conductor (macOS
only), amux. Cloud autonomy — Devin (reported ~68% PR acceptance and a
"babysitting tax"), Cursor background agents (cost scales with parallel agents,
vendor-locked), Codex cloud. OpenHands is self-hostable with weaker plan gates.
Nearest peers named by the orchestrator lists: Dex and LoopTroop. Sources:
[awesome-agent-orchestrators](https://github.com/andyrewlee/awesome-agent-orchestrators),
[SDD tools compared](https://www.augmentcode.com/tools/best-spec-driven-development-tools),
[background agents compared](https://amux.io/guides/background-agents-compared/),
[agentic-development requirements](https://arxiv.org/pdf/2609.04630).
These are aggregator-level sources; treat individual percentages as unverified.

## Where Surge is structurally ahead

1. **Human-approved plan before code**, with edit loop and stage sizing chosen by
   the planner (Aperant: none; Factory: a conversation; Kiro/Spec Kit: documents
   without a runtime).
2. **Verification that the implementer cannot write**: separate profile, sealed
   ledger effect, cross-vendor option, flow-load warnings for weak verification
   (Aperant's reviewer grades itself with the same tools).
3. **Event-sourced runs**: replay, fork, steer, crash recovery.
4. **Agent- and vendor-agnostic** (ACP) with delegated native sandboxes, against
   Aperant's denylist and Factory's hosted relay.
5. **MCP both ways**: supervised client and `surge mcp serve` for a driving agent.
6. **Local and inspectable**: no vendor control plane; costs are the user's own
   subscriptions.

## Where Surge is behind (and the plan)

| Gap | Seen in | Status |
|---|---|---|
| Agent-driven QA of the **running app** against the mission validation contract | Factory Missions | Contract exists in roadmap (`[[missions]]`); app-driving validator profile: this revision |
| Untrusted-content handling (prompt-injection) for tracker/issue text | 2026 security literature | This revision |
| Semantic merge-conflict resolution | Aperant | Open |
| Remote/mobile steering, cloud runners | Factory, Devin, Agentlas | Open (Surge is local) |
| Kanban queue, Insights chat, changelog generation | Aperant | Open |
| OpenTelemetry trace export | 2026 observability guidance | Open |
| Packaged installers for all platforms | Aperant, Agentlas | Open |
| Test-first enforcement and separate AI code-review pass | 2026 practice | `code-review` archetype exists; test-first not enforced |

## Reproducible evidence Surge has today

A recorded unattended run from a one-sentence idea to a verified working CLI, with
all five tasks and both milestones carrying evidence hashes:
[`recorded-e2e.md`](recorded-e2e.md). It is one project on one runtime; it is
evidence the loop closes, not a comparison.
