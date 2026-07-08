# Product strategy

Status: decision note, written 2026-07-07. Companion to the market survey in
[`agent-os-landscape.md`](agent-os-landscape.md) and the technical reference in
[`ARCHITECTURE.md`](ARCHITECTURE.md). This page records *what we bet on and in
which order*; the landscape page records *what everyone else is doing*.

## Positioning

**Surge is the cross-runtime completion layer for AI coding agents.** It owns a
structured task ledger, ungameable verification gates, and a replayable
event log over any agent runtime that speaks ACP.

> Agents solve tasks. Surge finishes projects.

Two user pains anchor everything below:

1. **Fleet interaction** — working with several live agents at once today means
   scattered terminal sessions, no unified "who needs me" view, and no way to
   steer one agent while others run.
2. **Large projects** — work that exceeds one context window forces the user to
   hand-slice specs/features into many sessions; state is lost between them and
   projects stall before completion.

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

Managed agents and first-party manager views try to complete a task inside one
vendor's walls. Surge is the local-first layer that makes fleets of agents
from any vendor finish whole projects — with a ledger that can't lose work, a
verifier that can't be gamed, and an event log that can replay how it happened.
