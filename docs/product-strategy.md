# Product strategy

Status: decision note, written 2026-07-07, updated 2026-10-05. Companion to the market survey in
[`agent-os-landscape.md`](agent-os-landscape.md) and the technical reference in
[`ARCHITECTURE.md`](ARCHITECTURE.md). This page records *what we bet on and in
which order*; the landscape page records *what everyone else is doing*.

## Positioning

**Surge is the cross-runtime completion layer for AI coding agents.** It owns a
structured task ledger, ungameable verification gates, and a replayable
event log over any agent runtime that speaks Agent Client Protocol (ACP).

> Agents solve tasks. Surge finishes projects.

Two user pains anchor everything below:

1. **Fleet interaction** — working with several live agents at once today means
   scattered terminal sessions, no unified "who needs me" view, and no way to
   steer one agent while others run.
2. **Large projects** — work that exceeds one context window forces the user to
   hand-slice specs/features into many sessions; state is lost between them and
   projects stall before completion.

## Product decisions — 2026-10-05

Recorded from a product interview with the maintainer. Where this section and
older text below disagree, this section wins.

- **Audience: people and developers.** Plain language by default, technical
  detail on request. Developers keep a complete CLI.
- **First contact: the macOS desktop app** with the daemon inside. The CLI and
  Telegram stay first-class, but the release is judged by the desktop path.
- **The promise: large projects get finished.** Verification, the task ledger
  and the event log are how Surge keeps that promise, not the headline. Demos
  show multi-milestone projects, not a single PR.
- **MCP recovery follows agent-runtime practice.** No managed VM or effect
  broker: the prior server's process group is stopped and a fresh server is
  started; an interrupted call is reported to the agent, never replayed
  ([ADR-0021](adr/0021-mcp-restart-recovery.md), superseding ADR-0020).
- **Local only.** No remote daemon and no hosted executor. During a run the
  desktop keeps the Mac awake and shows it.
- **Fully open source.** MIT/Apache-2.0, no telemetry, no paid tier; everything
  valuable lives in the open core.
- **First priority once the release is unblocked: prove the promise.** Run real
  multi-stage projects through the desktop and fix every failure they find. No
  curated demo recording is kept in the repository.
- **A project has no finale; it has stages.** Every stage the user asked for is
  driven to completion. New ideas can be added at any time and are completed the
  same way.
- **New idea during a stage:** the planner proposes where it goes in the roadmap
  (a task in a stage or a new stage); the user approves with one action and the
  current stage keeps running. An urgent bug takes the same path but may be
  proposed first in the current stage. Feedback on a finished stage is a new
  idea too.
- **End of a stage:** Surge continues to the next stage and notifies the user
  with a short report and a "run it" action. No acceptance gate by default.
- **A project opens on a dashboard:** "needs your decision" first, then stages
  with progress, recent activity, "add an idea" and "run the app". The
  conversation is reached from the dashboard.
- **Agent selection is automatic within user limits.** Surge detects installed
  agents and, where available, their models, limits and quality ratings, then
  assigns stages itself. Before approving a flow the user can change the
  assignment or ask for a revision. Providers or individual models can be
  disabled entirely, and allowed models, effort levels and thinking modes
  restricted. These limits constrain the planner and are validated at flow
  load, not only displayed. Ratings ship as registry data
  ([ADR-0019](adr/0019-providers-are-registry-data.md)) refreshed on the user's
  request; Surge never silently fetches third-party sites.

- **Two equal starting points:** "new project from an idea" and "open an existing
  folder". For an existing project Surge scans it (`project describe`), proposes
  stages and never touches the working copy; all work happens in worktrees,
  stated in one line when the project opens.
- **No agent installed or signed in:** the app guides step by step (supported
  agents, copyable install and sign-in commands) and switches screens itself
  once `surge doctor` sees the agent ready. Surge downloads nothing on its own.
- **Default agent permissions: project folder plus network** (`workspace+network`).
  Writes outside the worktree go through an elevation request in the Inbox. The
  verifier stays sealed (read-only, no network) regardless of this setting.
- **Notifications:** native macOS notifications by default; Telegram is opt-in
  in settings for replying from a phone.
- **Git is always visible** (branches, commits, diffs), with short hover hints
  for key terms instead of renamed concepts.

- **Subscription limit mid-stage:** the task moves to another allowed agent;
  if none fits, the run parks until the reset and wakes itself (ADR-0016). The
  user is notified; no decision is required. Allowed models, effort and
  thinking limits apply to the switch, and the verifier stays on a different
  vendor than the implementer where possible, with a warning otherwise.
- **Spend visibility: actuals only, no forecasts.** Show what the agent actually
  reports (share of a subscription limit where available, otherwise tokens);
  never invent dollar figures while ACP reports no cost.
- **Budgets are off by default** and can be enabled per project or stage in
  settings.

- **Repeated verifier rejection follows a ladder:** retry with a different or
  stronger allowed model, then the planner splits the task using the
  verifier's findings, then ask the user. Tasks that do not depend on it keep
  running. This implements the reserved `ExceededAction::Escalate` behind edge
  `max_traversals`.
- **The user can override a rejection** either by accepting as is or by revising
  the requirement and re-verifying. Accept-as-is is always shown as "accepted by
  a human", never "verified", and keeps the verifier's findings in the report.
- **Loop protection is independent of budgets:** a session with no progress
  (no new events, repeated identical tool calls, or a turn cap) counts as a
  failed attempt and enters the same ladder. Thresholds default sensibly and
  are configurable in `surge.toml`.

- **Several projects run in parallel by priority.** When limits are short, the
  project higher in a user-ordered list gets the agent; waiting work ages up so
  lower projects are not starved, and a project blocked on the user holds no
  agent share.
- **Each project is its own space.** There is no combined cross-project view;
  the user switches between projects. The app opens the last project, the
  switcher badges projects that need a decision, and native notifications come
  from every project. Fleet is scoped to the current project.

- **Planning is a step-by-step wizard.** An idea first gets 3–5 clarifying
  questions with defaults, then the user shapes each step: description, stages,
  agents and flow, start. A feature in a live project uses a short version (1–3
  questions, proposed roadmap place, approve); a small bug needs no questions.
- **The master agent plans, then steps back.** During planning it proposes the
  flow, including nodes with different viewpoints (for example a spec critic or
  a second tester); the user can also configure them. During execution it does
  not intervene in flows or agent sessions and only assists through MCP
  (context, memory, tools).
- **While work runs:** one line per task (who, which step, how long), expandable
  to the agent's live feed.
- **Course correction is stop and restart against a revised requirement**, not
  chatting with a running agent. The worktree's code is kept and the restart
  continues from it. Pillar B2 (steering without stopping) is not a desktop
  goal.
- **A two-layer second brain:** project memory in the repository
  (`.surge/memory`) and local personal memory carried across projects, both
  with provenance and editable; personal entries are supplied only when
  relevant. A local code index (file and symbol map) is refreshed at run start
  and served to agents through Surge's MCP; nothing leaves the machine.
- **Three ways to customize** skills, MCP, roles and flow templates: the UI, files
  and a conversation with the master agent. Files (TOML profiles, flows,
  skills, MCP) are the single source of truth; the UI edits them, and the master
  agent proposes a diff for approval and never applies it on its own.
- **First release cut: a reliable core, everything else in updates.** v1 (macOS,
  desktop) ships ADR-0021 MCP recovery, the idea/folder-to-stages path with the
  wizard, the ledger and verifier, limit parking, guided agent onboarding,
  native notifications and keep-awake; plus the project dashboard, new ideas
  during work, the verifier ladder with loop protection, and agent switching on
  limits with model/effort/thinking restrictions. After v1: benchmark ratings,
  cross-project priorities (equal share until then), Telegram in onboarding,
  budget UI, Windows and Linux.

- **Context inside a task is the runtime's job.** Surge's defense against context
  rot is task sizing at planning time and a fresh session per task; within a
  session the agent runtime's own compaction is used. A compaction is recorded
  and fed back to the planner as a signal to size similar tasks smaller.
- **Disagreement between viewpoint agents is settled by a judge agent**, on a
  different model or vendor than both sides where possible; its decision and
  rationale go into the stage report. Viewpoints are ordinary role profiles.
- **A Hub for skills and MCP servers.** The user installs from it; the planner
  picks per node only from what is installed; the user confirms in the wizard.
  Installs are hash-pinned, never silently updated, and MCP servers run under
  the delegated sandbox. Provisionally the Hub is a client over existing
  official catalogs (such as the official MCP registry and skill catalogs) plus
  the user's own skills and servers from a folder or git; Surge hosts and
  moderates nothing, and every entry shows its source.

- **Existing projects keep their conventions.** Surge reads existing agent
  instructions (`CLAUDE.md`, `AGENTS.md`, Cursor rules) as context and never
  replaces them.
- **Surge's project files live in the repository** under `.surge/` (memory, flows,
  profiles, `surge.toml`) and are committed; run journals and the code index
  stay local in `~/.surge`.
- **One PR per task; the user merges by default**, with auto-merge as a project
  setting. Without a GitHub remote the same PR exists locally as a branch with
  a review card in the app. Dependent tasks stack on unmerged PRs while
  independent tasks continue; the dashboard shows "N PRs waiting to merge" with
  a hint about auto-merge.
- **Conflicts with the user's own commits** are resolved by an agent and then
  re-verified; if the agent cannot resolve them, the rejection ladder applies.

Accepted risks: an agent can repeat a non-idempotent call after a crash despite
the notice; without stage gates a defect in one stage can surface after the
next stage builds on it; benchmark ratings age and may not reflect a given
project (local per-run history can complement them later, with no telemetry);
default network access lets an agent fetch anything (accepted for unattended
runs); command-only agent onboarding may be hard for people without a terminal
and should be tested with a real user; a switched agent finishes a task in a
different style; without a default budget an API-key user can overspend, so
loop protection must not depend on budgets; the
rejection ladder spends limits on a task that may be badly specified; "accept
as is" can become a habit; a low-priority project advances slowly until aging
lifts it; without a combined view, users with many projects rely on badges and
notifications; the wizard lengthens time to first result; stop-and-restart spends
an attempt on a small correction; three customization paths are three surfaces
to test; "no context loss" stays a differentiator only while vendors leave
context management to the user, so back it with Surge's own measurements on
large projects; trusting runtime compaction means a rare oversized task can
still degrade, so sizing quality must be measured; the judge is one more model
call and failure point; Hub quality depends on third-party catalogs; manual merging grows PR
stacks where a fix at the bottom shakes everything above; an agent can
misread a human change while resolving a conflict; dozens of PRs per stage add
noise to a team's GitHub; the v1 cut is sizable new code (`Escalate` and `Rotate` are
reserved today), so it, not MCP alone, sets the release date; first-party vendor tools may absorb "finishing projects", which would erode the
differentiation (see revisit triggers below).

## What the research established

Facts (survey dated 2026-07-07, sources in
[`agent-os-landscape.md`](agent-os-landscape.md)) that this strategy builds on:

1. **ACP-only transport is validated three independent ways.** Devin Desktop
   shipped with ACP as its cross-vendor interoperability layer; Omnara's
   post-mortem blamed CLI-wrapping ("unfeasible to maintain with Claude Code's
   constant updates"); BridgeMind orchestrates via PTY + output heuristics and
   its changelog is full of exactly that bug class. [ADR-0006](adr/0006-acp-only-transport.md)
   is confirmed — transport rigor is a survival variable.
2. **Thin orchestration is commercially dead.** Terragon shut down, vibe-kanban
   folded at 27k GitHub stars, Omnara archived — all absorbed by first-party
   vendor equivalents. "Parallel worktrees + a board" is not a product. The
   defensible layer is completion machinery: ledger integrity, verification,
   spec evolution, replayable runs.
3. **The completion architecture has converged.** Anthropic and OpenAI
   independently published the same long-horizon harness shape: *a durable
   ledger outside the context window + disposable fresh-context sessions per
   task + per-unit verification before a task flips to done*. Supporting data:
   context degrades well before window limits ("context rot"); METR's
   80%-success autonomy horizon is ~10× shorter than the 50% one; benchmark
   agents solve tasks but cannot reproduce full projects (Commit0: 6–29%).
4. **Reward hacking is not an edge case.** Cursor's audit found 63% of
   successful Opus 4.8 Max SWE-bench Pro resolutions were retrieved rather than
   derived; METR measured ~30% hacked runs on RE-Bench. "Loop until tests pass"
   is a gameable contract; false completion is *the* mechanism behind
   "projects never get finished".
5. **Manager views went first-party in 2026, and stayed vendor-siloed.**
   Anthropic Agent View, Antigravity Agent Manager, the Codex app, Cursor's
   Agents Window, Devin Desktop — each supervises only its own agents. The
   winning UX patterns are now known (inbox of blocked runs; worktree-per-run
   with promote-to-foreground; artifact-level batch review; steer-without-
   stopping; thread-per-agent remote approvals). A local-first, cross-vendor
   supervision surface with an audit-grade event log is open field.

## Principle: build, don't bridge

We do **not** ship adapters to third-party multiplexers (Herdr) or
agent-specific RPC surfaces (Pi RPC). Those tools stay *design references*;
their best ideas are implemented natively in Surge.

Rationale:

- Surge's state is **authoritative**: outcomes come from `report_stage_outcome`
  and ACP events, not from screen scraping or output heuristics. A cockpit
  rendered from the event log is strictly stronger than one reconstructed from
  a PTY — bridging would export our strongest signal into a weaker medium.
- Bridges create dependencies on fast-moving, single-author projects (Herdr:
  AGPL/commercial dual license, breaking protocol changes inside minor
  versions; Pi: unversioned single-vendor RPC, no capability negotiation).
- The graveyard evidence (fact 2) shows integrations with other people's
  surfaces get absorbed; native capability does not.

What we take as *reference, not dependency*:

| Source | Idea we implement natively |
|---|---|
| Pi | `steer` / `follow_up` queue semantics; branch summaries on fork; per-session token/cost introspection |
| Herdr | blocked-first triage ordering; toast/notification discipline; remote attach ergonomics |
| BridgeMind | git-committed project memory; curated per-task context packets; @-addressing any live agent |
| Anthropic/OpenAI harness posts | ledger + fresh-context + per-unit verification loop |
| OpenSpec | change-delta model that keeps specs from going stale |
| Devin | knowledge audit surface ("which memory items misled runs") |

## Pillar A — Completion machinery (the moat)

Answers pain 2. The user's manual spec-slicing is compensation for three
missing mechanisms; we build them.

### A1. Task ledger as the core product

`roadmap.md` today is prose — "write-only memory" that decays across sessions.
It becomes a typed, queryable ledger:

- Tasks with statuses, dependency edges, and a **`discovered-from`** edge —
  work found mid-task is captured instead of silently dropped (the thing that
  kills project completion).
- `surge ready` — query for unblocked work; the same query feeds safe
  parallel dispatch (dependency-disjoint tasks → concurrent runs).
- Bootstrap sizes tasks **to a context budget**: each ledger item must be
  completable in one fresh agent session. This automates the hand-slicing.
- Every session starts with a deterministic ritual: read ledger + git log +
  smoke checks before new work.

### A2. Ungameable verification

The strongest-evidence, least-shipped mechanism in the field.

- The Verifier node is the **sole write path** to a task's `done` status in
  the ledger; implementers can only report `ready_for_verification`.
- Verifier runs in a fresh context, never sharing the Implementer's session
  (already true by construction), with a **sealed sandbox intent**
  (read-only, no network) so it cannot look up solutions.
- Flow validation warns when a path reaches `Terminal: success` without a
  verifier node; "done with evidence" and "done without evidence" render
  differently everywhere.
- Verification evidence (test output, lint, screenshots, logs) is stored as
  artifacts and compiled into a **Run Report** attached to the PR — review the
  report in minutes instead of the transcript in hours.

### A3. Accumulating project memory

`project.md` is a per-run snapshot; nothing carries learning across runs.

- `.surge/memory/` — markdown memory committed to the repo alongside code,
  written at run boundaries, readable by every agent node via context
  bindings (never lazily mid-stage).
- Each entry records provenance (which run, which stage produced it).
- An audit surface — staleness is the universal failure mode of agent-written
  memory: `surge memory audit` correlates entries with failed/looping runs and
  proposes pruning.

### A4. Spec-delta evolution

Specs that go stale actively mislead agents. Surge's roadmap-amendment
machinery (`roadmap-patch.toml`, typed operations, conflict handling) is
already a change-delta model — extend it from roadmap structure to content
specs: completing a task includes merging its spec delta back into the
canonical spec, transactionally, as part of the flow. Ceremony scales with
task size (bug-fix archetypes get a lightweight path — no 16-criteria specs
for one-line fixes).

### A5. Fork with branch summaries

When fork-from-here abandons a prefix, summarize the abandoned branch and bind
the summary into the child run's context — cheap, and directly improves the
v0.3 fork/pre-fork-edit mechanism.

## Pillar B — Fleet interaction (native cockpit)

Answers pain 1. Every surface renders from the event log; nothing is scraped.

### B1. Inbox of blocked runs

The single strongest-converged UX pattern. `surge inbox` (CLI first) groups
every run/node by **Needs input / Working / Done**, ordered blocked-first.
Telegram gets a digest mode (batched card summaries) alongside per-card mode —
push vs. poll is user-configurable; approval fatigue is a documented failure
mode, so fewer, higher-stakes gates beat many small ones.

### B2. Steering without stopping

A Surge-level steering queue: the operator addresses any live node
(`surge steer <run> <node> "message"`, or @-addressing from Telegram), and the
message is delivered at the next safe boundary — emulated over ACP
(cancel + re-prompt with preserved session state) where the runtime lacks a
native primitive. `request_human_input` already covers agent→human escalation;
this adds the human→agent direction.

### B3. Native run cockpit

Live view of every active run: current node, last events, cost, pending
approvals — rendered from the event log (CLI/TUI first, desktop UI later).
Explicitly **not a terminal multiplexer**: we do not manage arbitrary PTYs; we
render our own authoritative state, which no multiplexer can reconstruct.

### B4. Promote to foreground

`surge run checkout <run_id>` — pull a run's worktree branch into the user's
working checkout in one gesture, for human takeover. The counterpart gesture
returns control to the run.

### B5. Attention as a budgeted resource

No shipped product models the human's focus. Long-term: batch non-critical
approvals into review windows, protect deep-work intervals, and let policy
decide what is worth an interrupt vs. the next inbox visit.

## Sequencing

| Phase | Ships | Why this order |
|---|---|---|
| 1 | A1 ledger + `surge ready` + context-budget task sizing; A2 verifier-owned `done` + sealed verifier sandbox | The completion moat; everything else composes with it |
| 2 | B1 inbox (CLI + Telegram digest); A3 `.surge/memory/` | Fleet triage becomes useful once the ledger feeds parallel dispatch |
| 3 | B2 steering queue; B4 promote-to-foreground; B3 cockpit TUI | Interaction depth on top of authoritative state |
| continuous | A4 spec deltas; A5 fork summaries; Run Report polish | Increments on existing machinery |

## Metrics

If the thesis is "Surge finishes projects", we measure ourselves on:

- **Completion rate**: fraction of started roadmaps that reach a merged,
  verified terminal state; time-to-reviewed-merge.
- **False-completion catch rate**: verifier rejections per 100 tasks (a healthy
  non-zero number is the feature working).
- **Intervention rate**: approvals + steers per run; time runs spend blocked on
  a human.
- **Evidence coverage**: fraction of completed runs with a full Run Report.
- **Memory health**: audit-flagged stale entries vs. total.

## Non-goals (reaffirmed and extended)

Everything in [`ARCHITECTURE.md` §13](ARCHITECTURE.md) stands. Additionally:

- **No bridges/adapters** to third-party multiplexers or agent-specific RPC
  protocols. Reference their ideas; implement natively.
- **No PTY management or screen-based state detection**, ever — state comes
  from ACP and the event log.
- **No proprietary coding agent** (BridgeCode's fate — announced, waitlisted,
  silently killed — is the cautionary tale).
- **No hosted control plane.** Local-first stays the form factor.

## Risks and revisit triggers

- **ACP adoption stalls or fragments.** Watch: runtime vendors' ACP support
  quality. Trigger to revisit ADR-0006's scope: a major runtime deprecates ACP
  while a materially better standard exists.
- **Pi ships first-party ACP** → treat Pi as one more runtime through the
  standard path (still no Pi-specific RPC connector).
- **A vendor manager view goes cross-vendor** (most likely Devin Desktop):
  sharpen the local-first + event-log-audit + completion-machinery
  differentiation; those remain unshipped anywhere.
- **Ledger ceremony backlash** (the Kiro "16 acceptance criteria for a one-line
  fix" lesson): adaptive complexity already picks flow shape per run — the
  ledger must inherit that (trivial fixes get trivial ledgers).

## Differentiation sentence

Updated 2026-10-05. Vendor tools can take a task from start to finish, but on
large work they hit the context window: it gets compacted, plans dead-end and
agents get lazier. Their answer is advice to the user — compact, start a new
session, split the task. In Surge that is the system's job, not the user's.
The brief is refined with the user before any code is written; the whole path
to the result is visible up front; every task runs in a fresh session against a
durable ledger, so context is never lost and work resumes whenever the user
wants, including after limits reset. Several agents from different vendors can
look at the same work from different angles, each node gets exactly the skills
and MCP servers it needs, and a project memory plus a code index make agents
faster on the user's project. All of it local, open source and from any vendor.
