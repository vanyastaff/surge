+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-09-07"
+++

# ADR 0018 — Nodes do not talk: Surge grows no agent mailbox

## Status

Accepted. This ADR records a **non-goal**. Nothing is being built; the point is
that a specific, currently fashionable thing will not be.

## Context

Every orchestrator-adjacent product shipped in 2026 has converged on the same
shape: give the agents a shared channel and let them talk. Claude Agent Teams
has a shared task list and mailbox; `yc-software/qm` puts agents in rooms;
`yetone/cumora` makes agents first-class members of a team chat; `dsh-agent-teams`
does the same inside DeepSeek Harness. The pattern is intuitive — a team of
humans coordinates by talking, so a team of agents should too — and it is
already table stakes in the surveyed cohort
(`docs/competitive-survey-2026-09-07.md` §1.1, §10).

Surge does not do this, and until now the reason was taste. There is now a
measurement.

*"Hallucination as Context Drift: Synchronization Protocols for Multi-Agent LLM
Systems"* (arXiv:2606.21666) ran 100 trials across two domains:

| Condition | Hallucination rate | vs. no sync |
|---|---|---|
| No synchronization | 0.492 | baseline |
| **Full broadcast** | **0.658** | **+34%**, *p* = 0.0022, *d* = 1.18 |
| Selective, divergence-gated | 0.463 | −5.9%, with 58% fewer API calls |

Broadcasting every agent's state to every other agent made results
**substantially worse**, not better. The mechanism is not subtle: one agent's
error propagates to all of them, and then all of them assert it in unison — so
the error acquires the appearance of independent corroboration, which is exactly
the signal a downstream reader uses to trust it. The contamination showed up in
the domain where a single wrong shared belief cascades across linked dimensions,
and not in the domain with orthogonal per-agent contexts. Coding work is the
first kind.

Two adjacent results point the same way. The handoff-hallucination taxonomy
(Springer, *"Handoff Hallucinations"*) reports Multi-Agent Debate at 10%
hallucination for 2.24× the cost of a per-boundary verification layer and with
the **lowest** epistemic fidelity of the methods compared — because each debater
summarises before the judge synthesises, and every summarisation is another
chance to strip the "we are not sure about this" while keeping the claim. And
the error-propagation study (arXiv:2608.14588) measures per-boundary escape
probabilities rising 24.6% → 48.3% → 89.3% as a claim is passed along and
re-narrated.

More talking is more boundaries. More boundaries is more transformation. The
literature is consistent: the unit of failure is the handoff, so the design
question is not *how much* agents should communicate but *what*, and *when*.

## Decision

**Surge nodes do not communicate with each other. There is no mailbox, no
broadcast channel, no shared scratchpad, and no debate round.**

What a node produces reaches another node exactly two ways:

1. **A typed artifact along a graph edge.** The contract is in
   `crates/surge-core/src/artifact_contract/kinds/`; the edge is
   `crates/surge-core/src/edge.rs`. What survives a boundary is whatever the
   schema forces to be present — which is the whole reason the artifacts are
   typed rather than prose.
2. **The durable event log**, which is the shared state. It is written once,
   read by anyone, and never re-narrated. A reader reconstructs; it does not
   receive someone's summary.

This is the blackboard pattern, and Surge already is one. The ADR exists so
that stays deliberate.

## Consequences

- **Accepted cost: no emergent coordination.** Two runs cannot notice each other
  and adapt. Where that coordination is genuinely needed — concurrent runs
  touching the same paths — it must be solved by a *gate over combined state*,
  not by letting the agents negotiate. That is a separate, unbuilt piece of
  work; this ADR does not deliver it, and explicitly does not accept "let them
  talk" as its substitute.
- **Accepted cost: we will look behind.** Competitors will ship agent chat and
  demo well. The counter-argument is a number, not a slogan, and it is in this
  file.
- **Gained: the low-contamination architecture by construction.** There is no
  channel through which one node's error can reach three others and come back
  wearing consensus.
- **Gained: the log stays authoritative.** A mailbox would create a second place
  where inter-node state lives, and the event log would stop being sufficient to
  reconstruct a run — which is the claim ADR-0006 and ADR-0017 both rest on.

## Alternatives rejected

- **Full broadcast / shared mailbox.** Measured at +34% hallucination. Rejected
  on the evidence above.
- **Multi-agent debate.** 2.24× the cost of per-boundary verification for a
  worse result and the lowest epistemic fidelity of the compared methods.
- **Selective, divergence-gated synchronization.** This is the one variant that
  measured *better* than no sync (−5.9%). It is rejected **for now** on scope,
  not on merit: it requires a divergence metric over agent state that Surge does
  not have and would have to invent, and −5.9% does not pay for that today. If
  it is ever built it belongs on the edge, as an edge policy, not as a channel.
- **A2A between nodes.** A2A (`docs/inter-agent-communication-2026-09.md` §2) is
  the right protocol for *another party's* agent addressing a Surge run from
  outside. It is the wrong thing between two nodes of one graph on one machine
  with one event log: it buys a negotiation we do not want and loses the log.

## Revisit triggers

- A replication of arXiv:2606.21666 that finds the broadcast regression does not
  hold for code-editing tasks specifically, or a result that isolates *which*
  shared content is contaminating and shows a safe subset.
- A concrete Surge failure that a mailbox would have prevented and a typed
  artifact plus the event log could not — stated as a run, not as an intuition.
- Divergence-gated synchronization becoming cheap because a divergence signal
  arrives for another reason.

## References

- `docs/inter-agent-communication-2026-09.md` §3.3 (the measurement), §4 (why
  the blackboard is the shape the evidence supports).
- `docs/competitive-survey-2026-09-07.md` §1.1, §10 (who ships the alternative).
- [ADR-0017](0017-run-report-is-a-log-projection.md) — the log is authoritative.
- [ADR-0006](0006-acp-only-transport.md) — Agent Client Protocol (ACP) as the
  sole agent transport.
