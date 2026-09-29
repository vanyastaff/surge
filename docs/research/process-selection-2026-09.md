# Process selection: how the planner picks the right shape of work

Status: research + proposal (2026-09-28). P0 and the Mission level (roadmap
`[[missions]]` with validation contracts and `fulfills`) are implemented; P1–P4
remain proposals.

Question: when Surge turns an idea into a roadmap, how should the planning
agents choose *the kind of process* for each level — roadmap, mission,
milestone, task — so that small work stays cheap and large or risky work is
still verified?

## 1. What Surge does today

Hierarchy: `RoadmapArtifact` → `RoadmapMilestone` → `RoadmapTask`
(`crates/surge-core/src/roadmap.rs`). No Mission level. Tasks carry `size`
(S/M/L, L = one fresh session), `depends_on`, `acceptance_criteria`,
`verified` (set only by a verifier).

The process is chosen **once per run**, by the Flow Generator (bootstrap stage
3, `bundled/profiles/flow-generator-1.0.toml`), as a single archetype for the
whole roadmap. Size rules: tiny → `single-task`, one task → `linear-3`, one
milestone → `linear-with-review`, ≥2 milestones → `multi-milestone`,
specialised work → specialised archetype.

Defects found while mapping this (verified in source):

| # | Defect | Where |
|---|---|---|
| D1 | Prompt offers 12 archetypes; `ArchetypeName` has 7. `feature`, `performance`, `security`, `docs`, `migration` cannot deserialize into `ArchetypeMetadata.name`, so picking them fails the generated flow. | `archetype.rs:22`, `flow-generator-1.0.toml:136-147` |
| D2 | A flow without `[metadata.archetype]` is accepted; `EngineError::ArchetypeMissing` is never constructed (ADR 0005 says it should reject). | `engine/validate.rs:365`, `engine/error.rs:121` |
| D3 | Topology is validated only for `multi-milestone`; `bug-fix` (reproduce first), `refactor` (characterise first) etc. fall through `_ => Ok(())`. | `engine/validate.rs:397-424` |
| D4 | One archetype for the whole run: a roadmap with a bug-fix task, a migration task and three feature tasks gets one shape. The specialised first steps (reproduce / baseline / audit) are lost inside `multi-milestone`. | design |
| D5 | Daemon L1 "Standard" path uses `MinimalBootstrapGraphBuilder` (single implementer); no process selection happens on the tracker path. | `surge-daemon/src/main.rs:1464` |
| D6 | The decision is prose in `metadata.description`; the signals behind it are not recorded, so it cannot be audited, replayed or learned from. | design |

## 2. What the field converged on

### 2.1 Planning depth scales with the work (BMAD, Kiro, Factory)

- **BMAD "scale-adaptive planning"**: four tracks. Trivial (edit → verify, no
  artifacts); one session (build plan); epic (spec → stories → build per story
  → integration → retro); project (≈20+ sessions or several epics: PRD, UX,
  architecture, shared contracts, epics). Escalate when earlier work reveals a
  missing constraint; de-escalate when a change fits one session.
- **Kiro specs**: three phases (requirements → design → tasks), two entry
  variants — *requirements-first* (behaviour is clear, design is not) and
  *design-first* (architecture is the constraint) — plus a *bugfix spec*
  (analysis → fix → no-regression) and a *quick spec* that skips gates for
  straightforward work. Independent tasks run in parallel "waves".
- **Factory Missions** (closest to Surge): Mission → Milestones (vertical
  slices that leave the product testable) → Features (one fresh worker each).
  Missions are for ~1–500 features; normal sessions for the rest. Cost floor:
  `runs ≈ features + 2 × milestones`.

### 2.2 Correctness is defined before the work is split (Factory)

The orchestrator writes a **validation contract** — a finite list of
behavioural assertions (`VAL-AUTH-001`: title, pass/fail condition, required
evidence) — *before* defining features, so the contract is not shaped by the
planned implementation. Coverage gate: every assertion is claimed by exactly
one leaf feature's `fulfills`; infrastructure features claim none. Milestone
boundaries run two independent validators: *scrutiny* (code review) and
*user-testing* (black-box use of the running app). Failures become **fix
features** inserted at the head of the queue; the Slack-clone case converged
in 2–4 validation rounds per milestone, fixes were 34% of implementation work.
Worker hand-off is structured: `successState`, `discoveredIssues`,
`whatWasLeftUndone`, `returnToOrchestrator` — the orchestrator must act on or
explicitly dismiss every item.

### 2.3 Coordination topology follows task structure

Anthropic "Building effective agents": prompt chaining (fixed decomposition),
routing (distinct categories, accurate classification), parallelisation
(sectioning / voting), orchestrator–workers (subtasks not predictable),
evaluator–optimizer (clear criteria, iteration pays), autonomous agent
(open-ended). Start with the simplest that works.

Claude Code dynamic workflows add six arrangements: classify-and-act,
fan-out-and-synthesize, adversarial verification, generate-and-filter,
tournament, loop-until-done — and name the three failures they fix: agentic
laziness, self-preferential bias, goal drift. They cost several times a
normal session; "a two-line bug fix does not justify" one.

Anthropic multi-agent research: effort scaling (1 agent / 3–10 calls for
simple; 2–4 subagents for comparisons; 10+ for broad research), ~15× chat
tokens, subagents write to the filesystem and pass references. It explicitly
**does not fit coding with shared context or heavy inter-agent dependencies**.

### 2.4 Swarms ("рой")

- **OpenAI Swarm**: agents + routines + handoffs (a tool call that returns the
  next agent; shared history). Educational; one agent active at a time. Good
  for triage/routing, not for building.
- **claude-flow / Ruflo hive-mind**: queens (strategic / tactical / adaptive)
  over typed workers (researcher, coder, architect, tester, reviewer…);
  topologies mesh, hierarchical, ring, star; shared SQLite memory.
- **Stigmergy / blackboard**: agents coordinate by writing to a shared board
  (claim locks, decaying signals) instead of messaging each other. Works when
  specialists contribute at different rates and the problem benefits from
  revision passes.
- **CodeCRDT**: parallel agents editing one codebase via CRDTs — speedup only
  on complex, decomposable tasks; simple tasks get slower from coordination.
- **Failure data**. MAST (1,600+ traces, 14 modes): root causes are
  specification ambiguity, coordination breakdown, verification gaps.
  Swarm-specific: semantic/coordination/behavioural drift, duplicate work
  without a central registry, O(n²) failure surface (4 agents → 6 edges, 10 →
  45), quality drops after 8–10 sequential handoffs. Supervisor-specific: lossy
  summaries between levels, saturation after 8–12 round trips, over-centralised
  prompts with 5+ responsibilities. OrchestraBench: tool errors recover (1.0),
  but context pollution, conflicting sub-agent outputs and premature action do
  not recover at all (0.0) — **retry reproduces latent faults**; explicit
  decomposition gave 1.00 delegation fidelity vs 0.37 monolithic.
- **Cognition "single writer"**: many agents may contribute intelligence;
  writes stay single-threaded. Anthropic and Cognition agree once you split by
  task structure: isolation wins on independent threads, loses on shared state.

Consensus for 2026: **supervisor is the default**; swarm/mesh only for
read-heavy exploration and triage; parallel writers only in isolated
worktrees on disjoint scopes, merged by one owner.

### 2.5 Human-process analogues worth borrowing

Shape Up *appetite* (a fixed budget chosen before the solution, scope is cut
to fit) maps onto a per-milestone budget cap. Spikes / tracer bullets /
walking skeleton map onto `spike` and onto "milestone 1 is the thinnest
end-to-end slice". Archify's five diagram languages (architecture, workflow,
sequence, data flow, lifecycle) are a useful checklist for which design
artifacts a design-first milestone should produce.

## 3. Proposal: choose along four axes, per level

Today one label (`archetype`) carries everything. Split the decision into
orthogonal axes, each chosen at the level where the information exists.

| Axis | Values | Chosen by | Signals |
|---|---|---|---|
| **Scale** (planning depth) | `trivial`, `task`, `milestone`, `mission`, `program` | Description/Roadmap planner | estimated sessions, number of vertical slices, cross-system reach, number of external contracts |
| **Kind** (what makes it checkable) | `feature`, `bug-fix`, `refactor`, `migration`, `performance`, `security`, `docs`, `spike`, `ui` | Roadmap planner, per task | verb of the goal, presence of a failing case, "no behaviour change", data/schema change, measurable target, unknowns |
| **Entry** (what is specified first) | `requirements-first`, `design-first`, `spike-first`, `quick` | Roadmap planner, per milestone | clarity of behaviour vs clarity of architecture vs unknown feasibility |
| **Topology** (how many agents) | `single`, `pipeline`, `fan-out` (parallel worktrees), `tournament` (best-of-N), `loop-until-green` | Flow Generator, per milestone/task | independence of tasks (disjoint file scopes, no `depends_on`), uncertainty of design, cost budget |

Verification strength is derived, not chosen: every level gets a verifier;
`mission` and `program` additionally get a validation contract and
milestone-boundary scrutiny + user-testing.

### 3.1 Scale rubric (what the planner checks first)

| Scale | Rule | Artifacts | Flow |
|---|---|---|---|
| trivial | one area, low risk, obvious change, < 1 session | none | `single-task` + verifier |
| task | one coherent change, fits one fresh session (size ≤ L) | acceptance criteria | kind-specific pipeline (`linear-3`, `bug-fix`, …) |
| milestone | one user-visible outcome, 3–15 tasks | description, roadmap with 1 milestone | task loop + milestone verifier + review |
| mission | 2+ vertical slices, or ~15–100 tasks, or a new app | description, **validation contract**, roadmap | `multi-milestone` with contract coverage gate |
| program | several missions / independent products / 20+ sessions per mission | roadmap of missions, shared contracts | one mission run at a time, human gate between missions |

Escalate when a task's handoff reports `whatWasLeftUndone` twice, when
discovered tasks exceed ~30% of the milestone, or when a milestone needs more
than 4 validation rounds. De-escalate when the roadmap planner produces a
single milestone with ≤ 2 tasks.

### 3.2 Kind rubric (first step that makes the rest checkable)

| Kind | Trigger signals | Mandatory first step | Mandatory last check |
|---|---|---|---|
| feature | new behaviour | acceptance criteria / assertions | assertions pass on running app |
| bug-fix | a reproducible failure | failing test that reproduces it | test green + no regression |
| refactor | "no behaviour change" | characterisation tests | same tests green, diff reviewed |
| migration | schema/data/API version change | migration plan + rollback | forward + rollback validated |
| performance | a number to beat | baseline measurement | benchmark vs baseline |
| security | vuln, auth, secrets | audit | regression tests for each finding |
| docs | documentation-only | outline | review against code |
| spike | unknown feasibility | time/turn budget + question | written answer, no production merge |
| ui | user-facing surface | flow/sequence sketch | user-testing validator |

These map one-to-one onto the bundled flows, so the Kind axis is the existing
archetype list minus the size-driven ones (`linear-3`, `linear-with-review`,
`multi-milestone`, `single-task` are Scale, not Kind).

### 3.3 Topology rubric (swarm only where it is safe)

1. Default **single writer per worktree**, supervisor = the engine graph.
2. **fan-out** only for tasks in the same milestone with no `depends_on`
   between them and disjoint declared file scopes; each in its own worktree;
   merge serialised by one integrator node; cap parallelism (e.g. ≤ 4).
3. **tournament** (N implementers, one judge) only for `spike` or
   design-first milestones where the design is uncertain and cost allows it.
4. **Read-only swarm** (blackboard of findings) is allowed for scouting,
   research and review — never for writes.
5. Never more than ~8 sequential agent handoffs per milestone body; beyond
   that, split the milestone.
6. On latent faults (conflicting outputs, context pollution, premature
   action) do not retry the same node — escalate or re-plan (OrchestraBench).

### 3.4 Record the decision as data

Replace the prose-only `metadata.description` with a structured block the
validator can check and the UI can show:

```toml
[metadata.process]
scale = "mission"
entry = "requirements-first"
topology = "pipeline"
signals = ["3 vertical slices", "new app", "no failing case", "tasks share schema"]
rejected = [{ option = "fan-out", reason = "m1 tasks all depend on the schema task" }]
```

and a per-task `kind` on `RoadmapTask` (planner sets it; Flow Generator routes
by it inside the task loop via a `Branch` on `loop_item.kind`).

## 4. Rollout

| Phase | Change | Fixes |
|---|---|---|
| P0 | Make enum, prompt, bundled flows and ADR 0005 agree (add the 5 variants or drop them from the prompt); raise `ArchetypeMissing`; topology checks per archetype (first node of `bug-fix` is a reproduce stage, etc.); test that every bundled flow parses and declares its archetype. | D1–D3 |
| P1 | Scale/Kind/Entry rubric in the description + roadmap planner prompts; `kind` on `RoadmapTask`; structured `[metadata.process]` with signals; UI shows it next to the diagram at the flow gate. | D6 |
| P2 | Per-task routing inside `multi-milestone`: task loop body branches on `kind` into the kind-specific subgraph. | D4 |
| P3 | Validation contract for `mission` scale: `validation-contract.toml` written before the roadmap, `fulfills` on tasks, coverage gate in the validator, milestone-boundary scrutiny + user-testing verifiers, fix tasks at queue head. | — |
| P4 | Mission level above milestones; fan-out topology with worktree-per-task and serialized integrator; tournament for spikes; daemon L1 uses the real bootstrap or at least the Scale/Kind classifier. | D5 |

## Sources

- Claude Code dynamic workflows — https://claudefa.st/blog/guide/development/dynamic-workflows
- Anthropic, Building effective agents — https://www.anthropic.com/research/building-effective-agents
- Anthropic, How we built our multi-agent research system — https://www.anthropic.com/engineering/multi-agent-research-system
- Factory Missions overview — https://docs.factory.ai/missions/overview
- Factory Missions planning & validation — https://docs.factory.ai/missions/planning
- Factory, How Missions work — https://factory.com/news/missions-architecture
- Factory mission orchestrator prompt (reverse-engineered) — https://gist.github.com/V1ki/356b121038722ebf32b5aac85482c113
- Kiro specs — https://kiro.dev/docs/specs/
- BMad Method, choose a planning path — https://docs.bmad-method.org/plan/choose-a-planning-path/
- OpenAI Swarm — https://github.com/openai/swarm
- Claude Flow hive-mind — https://github.com/ruvnet/ruflo/wiki/Hive-Mind-Intelligence
- Augment Code, Swarm vs supervisor — https://www.augmentcode.com/guides/swarm-vs-supervisor
- OrchestraBench — https://arxiv.org/html/2608.05263v1
- CodeCRDT — https://arxiv.org/pdf/2510.18893
- Cognition, Don't build multi-agents — https://cognition.com/blog/dont-build-multi-agents
- Blackboard multi-agent LLM systems — https://arxiv.org/pdf/2507.01701
- Archify — https://tt-a1i.github.io/archify/
