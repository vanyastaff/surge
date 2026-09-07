# Communication between models and agents — protocols and evidence, September 2026

Status: research note, written 2026-09-07. Companion to
[`competitive-survey-2026-09-07.md`](competitive-survey-2026-09-07.md), which found that
cross-vendor verification (a diff written by one vendor's model, checked by another's) is
the one capability only an agent-agnostic orchestrator can offer. That idea only works if
context can cross a model boundary without rotting. This page is about whether it can, and
how.

Two different questions get conflated under "how do agents talk to each other":

- **The protocol question** — what wire format carries a message. Mostly settled, and
  mostly not where the difficulty is.
- **The method question** — what you put in the message so the receiver acts correctly.
  This is where the failures are, and there is now a real empirical literature on it.

All research findings below are attributed to their source with an identifier and are
reported as that source's claim, not as verified behaviour. Nothing here is a dependency
proposal.

---

## 1. First, a naming trap that will bite our own docs

**"ACP" means two unrelated things in 2026, and both appear in coding-agent contexts.**

| Name | Who | What it connects | Relevance |
|---|---|---|---|
| **Agent *Client* Protocol** | Zed + JetBrains, neutral GitHub org | an editor/client ↔ a coding agent, JSON-RPC over stdio | **this is the one ADR-0006 commits us to** |
| **Agent *Communication* Protocol** | IBM / BeeAI, later AGNTCY (Cisco, LangChain, LlamaIndex, Dell, Oracle, Red Hat) | agent ↔ agent, REST/OpenAPI over HTTP | unrelated to us; reportedly folded into A2A in August 2025 |

Third-party surveys of "agent protocols" almost always mean the second one. A survey of 200
engineering leaders (OSSA, January 2026) reporting "ACP: 31% awareness, 9% active use, 2%
production" is measuring IBM's protocol, not ours. Quoting those numbers against ADR-0006
would be a category error. Our docs should say **"Agent Client Protocol (ACP)"** in full on
first use, every time.

## 2. The protocol layer is settled, and it is two layers

The consensus across the surveys read for this note is that the protocols are
complementary, not competing, and that the reference architecture is two axes:

- **Vertical — MCP (agent ↔ tools).** Anthropic, now under the Linux Foundation's Agentic
  AI Foundation. Reported at roughly **97M monthly SDK downloads** and close to **20,000
  indexed servers**. Its **28 July 2026** revision is the largest since launch: the
  stateful core is removed — no `initialize` handshake, no session header — so servers sit
  behind ordinary HTTP load balancers without sticky sessions, plus MCP Apps
  (server-rendered interfaces) and a Tasks extension for long-running work.
- **Horizontal — A2A (agent ↔ agent).** Google, also a Linux Foundation project. Reached
  **v1.0 with cryptographically signed Agent Cards**, discovery at `/.well-known/`, a
  native long-running task lifecycle, and reportedly **150+ organisations in production**
  (Microsoft, AWS, Salesforce, SAP, ServiceNow). The far side of an A2A call is not a tool
  — it is an autonomous system that may accept, ask a clarifying question, run for hours,
  or refuse.

The rest: **AG-UI** (agent → frontend streaming, CopilotKit), **ANP** (DID-based
decentralised networking, early). Cross-protocol interoperability is explicitly unsolved —
an A2A agent cannot natively delegate to an IBM-ACP agent inside one task lifecycle; a
joint MCP/A2A interoperability specification was slated for Q3 2026.

**Read for Surge.** The three axes are orthogonal and we already sit on two of them:
Agent Client Protocol downward to runtimes, MCP outward to tools (per-run scoped and
supervised, [ADR-0014](adr/0014-mcp-server-lifecycle.md)). The axis we do **not** occupy is
A2A — which is the right one if Surge is ever to be *addressable by* someone else's agent
rather than only driving agents itself. That is the same conclusion the survey reached from
the product side (Superset ships an MCP server, an OpenAPI description and an A2A agent
card so agents can drive it). It is a distribution decision, not an architecture one, and
it does not touch ADR-0006.

**What A2A is not.** It is not how a Surge implementer node hands work to a Surge verifier
node. Those are two nodes in one graph on one machine with one event log; putting an
HTTP peer-to-peer negotiation protocol between them would buy nothing and lose the log.
See §4.

---

## 3. The method layer: five findings that should change designs

The interesting result of this search is that the 2026 literature has stopped asking "which
framework" and started measuring **the handoff itself as the unit of failure**. Five
findings, each with numbers, each with a direct consequence for us.

### 3.1 When you verify matters more than whether you verify

*"The Hallucination Snowball: Modeling Error Propagation as State Transitions in Multi-Agent
LLM Pipelines"* (arXiv:2608.14588, June 2026) instruments a 4-agent pipeline with 346
injected hallucinations and reports that errors do not merely persist across handoffs —
they **transform**, through Raw Fact → Derived → Narrative → Invisible, with measured
per-boundary escape probabilities of **24.6% → 48.3% → 89.3%**. Detection by the same
detector falls from 72.0% at stage 1 to 50.9% at stage 4; 23.7% survive to the final output
entirely undetected.

The headline result is about *placement*, not detector quality:

| Strategy | Hallucination survival |
|---|---|
| No verification | 60.7% |
| **End-of-pipeline** checking | 58.4% (**+2.3pp** — statistically negligible) |
| **Boundary gates** at each handoff, *same tools* | **16.2%** |

Reported at Cohen's *h* = −0.911, *p* < 0.000001 across five independent tests. Their
explanation is structural: by the time an end gate runs, 89.3% of hallucinations are
embedded in narrative and the original checkable claim no longer exists in verifiable form.
Their prescription is to spend the verification budget at the **first** boundary, where
75.4% is still catchable.

**Consequence for us.** This is the strongest external validation the architecture has
received, and it is an indictment of the pattern the entire competitor field uses. Every
product in the competitive survey reviews **at the end**: the agent works, then a human or
a bot reads the PR. That is the 2.3pp column. Surge's verifier-node-per-boundary model is
the 16.2% column, and the "review the plan before code" gate is exactly the *first*
boundary the paper says to spend on.

It also carries a warning: a graph whose only verifier sits at the end is, by this
evidence, close to having none. That is checkable statically, and belongs next to the
twenty-one validation rules.

### 3.2 Handoff packages must carry epistemic status

*"Handoff Hallucinations: Taxonomy, Benchmark, and Mitigation for Multi-agent Pipeline
Failures"* (Springer) formalises the handoff event as the unit of analysis and splits
failures by origin:

- **Type I — intrinsic:** the sending agent hallucinated.
- **Type II — extrinsic:** introduced *by the handoff transformation itself*.
- **Type III — context-collapse:** caused by **stripping epistemic markers during
  compression** — the content survives, the "we are not sure about this" does not.

Their mitigation is a per-boundary Handoff Verification Layer (consistency check with
recursive grounding retrieval, confidence calibration, an authenticity scorer, an adaptive
compressor), reported at near-zero hallucination for **1.39× the token cost** of the
unmitigated baseline, dominating Reflexion and Multi-Agent Debate on the cost/accuracy
frontier. Multi-Agent Debate specifically is reported at 10% hallucination for 2.24× the
HVL's cost with the *lowest* epistemic fidelity, because each debater summarises before the
judge synthesises — every summarisation is another chance at Type III.

They name the design principle **verified communication**: a handoff package must carry
detectable signals of epistemic status, sufficient for the receiver to reason under
uncertainty.

**Consequence for us.** We already do the core of this, which is worth noticing.
`MemoryClaim` carries a `Confidence` (`Verified` → `NameMatched` → `Asserted`) and
`ContextPack::build` admits candidates **in confidence order, never promoting a cheaper
low-confidence claim ahead of a higher-confidence one**. That is an epistemic status signal
surviving a compression boundary, which is precisely the Type III defence. The gap is that
this discipline lives in *memory* and nowhere else: the `verification-report` artifact
requires only `task_id` and `outcome`
(`crates/surge-core/src/artifact_contract/kinds/verification_report.rs`), and node-to-node
artifacts carry no confidence at all.

### 3.3 More communication makes multi-agent systems worse

*"Hallucination as Context Drift: Synchronization Protocols for Multi-Agent LLM Systems"*
(arXiv:2606.21666) is the most counterintuitive result found, and the most useful one for
deciding what **not** to build. Across 100 trials in two domains:

| Condition | Hallucination rate | vs. no-sync |
|---|---|---|
| No synchronization | 0.492 | baseline |
| **Full broadcast** | **0.658** | **+34%**, *p* = 0.0022, *d* = 1.18 |
| Selective, divergence-gated (SSVP) | 0.463 | −5.9%, and **58% fewer API calls** than broadcast |

Broadcasting every agent's state to every other agent made things substantially *worse*,
because one agent's injected error propagated to all of them, and all three then asserted
it in unison. The authors' framing: *"The right design question is not 'how much should
agents communicate?' but 'what should agents communicate, and when?'"* The contamination
effect appeared in the domain where a single wrong shared belief cascades across linked
dimensions, and not in the domain with orthogonal per-agent contexts.

**Consequence for us.** This is direct evidence against the shape that Claude Agent Teams
(shared task list and mailbox), `yc-software/qm` (agents in channels), `yetone/cumora`
(team chat with agents as members) and `dsh-agent-teams` all adopt. It is also a reason to
be explicit that Surge is *not* going to grow an agent mailbox. Our nodes do not talk; they
pass typed artifacts along graph edges, and the event log — not a broadcast — is the shared
state. That is the low-contamination architecture by construction, and until now we have
had no argument for it beyond taste.

### 3.4 Compression turns "must" into "maybe" — and there are four fields that stop it

*"When 'Must' Becomes 'Maybe': Constraint Weakening in LLM Agent Workflows"*
(arXiv:2608.24569, August 2026) is the most immediately actionable paper in this set. Across
1,296 controlled episodes it studies what happens when upstream state is turned into
"summaries, plans, tickets, memories, and handoff notes" that downstream components act
from. Conditioning on *correct* upstream identification — the sender got it right — it
reports:

> "Normal handoff compression produces **100.0% deactivation** and **54.2% forbidden
> action**." … "Restoring all four state fields raises preservation to **100.0%** and
> reduces forbidden action to **0.0%**." … "downstream verification eliminates forbidden
> action while artifact deactivation remains **95.3%**."

The four fields, quoted: **prerequisite, authority, fallback, and execution consequence**.

Two things there are worth separating. First, topical retention is not enough: an artifact
can *mention* an unresolved blocker while silently demoting it from "must be resolved before
execution" to "context that may inform the next action". Their term is operational state
preservation, and the summary is *"semantic availability does not guarantee operational
preservation."* Second, and this is the part that matters for a system that already has
gates: **downstream verification contains the damage but does not repair the artifact** —
forbidden action goes to zero, deactivation stays at 95.3%. A verifier stops the bad action
this time; the constraint is still gone from the artifact for everyone downstream.

**Consequence for us.** Every artifact in
`crates/surge-core/src/artifact_contract/kinds/` is a handoff artifact in exactly this
sense — `plan`, `spec`, `requirements`, `story`, `roadmap_patch`, `discovered_tasks`,
`verification_report`. None of them has a place to put "this is a prerequisite, this is who
may waive it, this is the fallback, this is what happens if you proceed anyway." The
approval machinery (`approvals.rs`) is all *policy* — which channel, what timeout, whether
elevation is allowed — and carries none of the four fields either. This is a concrete,
cheap, testable change with a measured effect size behind it.

### 3.5 Budgeted compression destroys constraints before it destroys facts

*"Facts Without Rules: Boundary Metadata Collapse in Multi-Agent LLM Handoffs"*
(arXiv:2608.29028, August 2026) measures the same phenomenon from the privacy side and
finds the two survive **independently**: boundary-marker survival and operational-fact
survival are near-uncorrelated (Pearson *r* ≈ 0 on both models tested). Uncompressed
free-text handoffs preserve boundary markers at 0.80; imposing a **25-word budget drops
that to 0.57 while fact survival stays near ceiling**. Vague constraint language leaked in
73% of cases on one model; making the constraint explicit cut leakage below a small
threshold across all three models tested, and a correctly derived audience allowlist nearly
eliminated it.

**Consequence for us.** Wave 2's context pack has a hard token budget with a receipt
recording what was selected, what was dropped and why (`A08`). This finding says the budget
is not a neutral squeeze: under pressure it eats the *rules* before it eats the *facts*.
So the receipt should not merely record that something was dropped — it should be able to
say **a constraint-bearing claim was dropped**, which is a different and more alarming
event than dropping a fact. Same lesson applies to `compaction` anywhere it lands.

### 3.6 A cheap intervention: ask at the edge

*"AgentAsk: Multi-Agent Systems Need to Ask"* (ACL 2026) gives an edge-level error taxonomy
— **Data Gap, Signal Corruption, Referential Drift, Capability Gap** — and shows that a
lightweight module that inserts a *clarifying question* at selected edges improves accuracy
by up to 4.69% while keeping added latency and cost under 10%. Related: MAST (Cemri et al.)
catalogues 14 failure modes across 1,600+ annotated traces from seven frameworks and finds
inter-agent misalignment to be a distinct, pervasive category rather than a sum of
individual model errors.

**Consequence for us.** Surge has edges as first-class objects with a policy
(`Edge { kind, policy }` in `crates/surge-core/src/edge.rs`) and already models a
`HumanGate` `edit` outcome routing backwards via `EdgeKind::Backtrack`. An edge policy that
can require a clarification before traversal is a small addition to a structure that
already exists, and the taxonomy gives four named conditions to trigger on.

---

## 4. So how *should* two different vendors' models exchange work?

Pulling the five findings together, the answer the evidence supports is nearly the opposite
of "let the agents talk":

1. **Do not connect them directly.** No mailbox, no broadcast, no debate. §3.3 measures
   that as a 34% regression. Connect them through a **durable artifact plus an event log**,
   which is the blackboard pattern and which Surge already is.
2. **Make the artifact typed, not prose.** Free text is where Type II and Type III damage
   happens. Both the constraint-weakening and boundary-collapse papers show that what
   survives compression is whatever the *schema* forces to be present.
3. **Carry epistemic status on every claim**, and never let a compression step reorder or
   drop by cost alone (§3.2). We do this in `ContextPack` and nowhere else.
4. **Carry the four operational fields on anything that constrains action** — prerequisite,
   authority, fallback, execution consequence (§3.4).
5. **Gate at the earliest boundary, not at the end** (§3.1) — and remember a gate *contains*
   the damage without *repairing* the artifact (§3.4), so gating does not remove the need
   for 2–4.
6. **Ask, cheaply, at the edge**, when one of the four edge conditions is detected (§3.6).

For cross-vendor verification specifically — the C08/C09 idea — this says the verifier
should **not** receive the implementer's transcript. It should receive the diff, the
originating spec with its constraints intact, and the evidence bundle, as typed artifacts.
A transcript is the compressed narrative the snowball paper says is already 89.3%
unverifiable; the diff plus the constraints is the raw-fact stage where 75.4% is still
catchable. That is a design claim we can now source rather than assert.

---

## 5. What to do about it

| # | Change | Grounded in | Lands on |
|---|---|---|---|
| 1 | **Four operational fields on constraint-bearing artifacts** — prerequisite, authority, fallback, execution consequence | §3.4 (100% → 0% forbidden action) | `artifact_contract/kinds/*`, `approvals.rs` |
| 2 | **Validation rule: a graph whose only verifier is terminal is flagged** — verification at the last boundary is measured at +2.3pp over none | §3.1 | `engine/validate.rs`, next to the 21 rules |
| 3 | **Context-pack receipt distinguishes dropping a constraint from dropping a fact** | §3.5 (survival near-uncorrelated; budget eats rules first) | Wave 2 receipt |
| 4 | **Epistemic status beyond memory** — confidence on node-to-node artifacts, not only on `MemoryClaim` | §3.2 (verified communication) | `verification_report` and siblings |
| 5 | **Cross-vendor verifier receives artifacts, never the transcript** — written into the C08 design, with the reason | §3.1 + §3.4 | Wave 5 C08 |
| 6 | **Edge clarification policy** on the four named edge conditions | §3.6 (+4.69% at <10% cost) | `EdgePolicy` |
| 7 | **Record the non-goal: Surge does not grow an agent mailbox** | §3.3 (+34% under full broadcast) | ADR |
| 8 | **A2A agent card** so other people's agents can address a Surge run | §2 | distribution, not architecture; does not touch ADR-0006 |
| 9 | **Write "Agent Client Protocol (ACP)" in full on first use** everywhere | §1 | docs hygiene |

Items 1–4 and 6 are small changes to structures that already exist. Item 7 is free and
prevents a fashionable mistake. Item 5 costs nothing now and prevents the C08 design from
defaulting to "hand the reviewer the transcript," which is the intuitive choice and,
by this evidence, the wrong one.
