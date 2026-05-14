+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-05-13"
+++

# ADR 0013 — Tracker automation tiers (L0 / L1 / L2 / L3)

## Status

Accepted.

## Context

Surge ingests tickets from external trackers (GitHub Issues, Linear) through a
`TaskSource` trait. By the start of this milestone, the path was effectively
"one size fits all": every observed ticket went through the full Description →
Roadmap → Flow bootstrap and required a human approval click before any run
started. That was correct as a safe default, but two operator-side needs
showed up immediately:

- **Skip-the-LLM tickets.** A ticket the operator does not want surge to touch
  at all (housekeeping, duplicate bug reports, tickets owned by another team)
  should never pay the triage-author LLM cost. The natural way to signal this
  is a tracker label — but the daemon was running triage on every ticket
  regardless.
- **Trust-the-template tickets.** A repeatable workflow (e.g. "add a Rust
  crate scaffold", "tag a release") should not have to redo the full bootstrap.
  The template already encodes the desired pipeline; the user wants the run to
  start directly off it.

A third need, **trust-the-whole-pipeline tickets** (auto-merge on green
checks), is also valuable for the most disciplined workflows but is the
hardest to ship safely.

The ROADMAP milestone `Tracker automation tiers` codified the four-tier model:
L0 (disabled), L1 (full bootstrap + approval, default), L2
(`surge:template/<name>`, skip bootstrap), L3 (`surge:auto`, full automation
including merge).

## Decision

Surge models tracker-automation policy as a closed enum
`AutomationPolicy { Disabled, Standard, Template { name }, Auto { merge_when_clean } }`
resolved deterministically from a ticket's labels. The resolved policy is
threaded through three observable surfaces:

1. **Triage gate** (`surge-daemon::handle_triage_event`). L0 tickets short-
   circuit before triage-author is dispatched; the daemon writes a
   `Skipped` row in `ticket_index` with `triage_decision = "L0Skipped"` and
   returns. L1/L2/L3 fall through into triage.
2. **Inbox dispatch** (`surge-daemon::dispatch_triage_decision`). The
   `Enqueued` arm branches on the resolved tier:
   - L1 (`Standard`) → existing inbox-card path (user approves Start).
   - L2 (`Template { name }`) → synthesize an `InboxActionRow { kind: Start,
     decided_via: "auto", policy_hint: Some(name) }` directly; skip the
     visible card and the operator click.
   - L3 (`Auto`) → identical to L1 for the inbox-card leg (operator still sees
     the card for visibility) but the resulting run is flagged for the L3
     auto-merge gate after `RunFinished { Completed }`.
3. **Merge gate** (`surge-daemon::automation_merge_gate`). A new consumer
   subscribes to `GlobalDaemonEvent::RunFinished` and, for L3 tickets,
   evaluates merge readiness and posts a `merge-proposed` or `merge-blocked`
   comment + label.

Idempotency is centralized in a new `intake_emit_log` table keyed by
`(source_id, task_id, event_kind, run_id)`. Every outbound side-effect that
must survive daemon restarts records into this table; retries no-op.

## Rationale

1. **Closed enum over open string set.** Tracker labels are a free-form
   namespace (any user can add `surge:experimental`). Mapping them to a
   closed `AutomationPolicy` enum at one canonical site keeps the decision
   logic out of every consumer's parser. The `#[non_exhaustive]` attribute
   on the enum reserves room for future tiers without forcing every match
   site to break.
2. **Precedence is explicit, total, and tested.** `surge:disabled` >
   `surge:auto` > `surge:template/<name>` > `surge:enabled` > none ⇒
   `Disabled`. Five proptests in `surge_intake::policy` verify the table
   is deterministic and that more-restrictive labels win over less-restrictive
   ones regardless of label ordering.
3. **L0 short-circuit pays for itself.** Triage-author is the most expensive
   surge-side per-ticket cost. Short-circuiting before it runs is the single
   biggest cost lever for operators who use surge across noisy trackers.
4. **L2 reuses the same launcher as L1.** The `TicketRunLauncher` helper in
   `surge-daemon::inbox` is the single spot that knows how to seed
   `project_context` and start a run. Both L1 (after operator click) and
   L2 (synthesized auto-Start) feed through it; the only branch is whether
   the graph comes from `BootstrapGraphBuilder::build` or
   `ArchetypeRegistry::resolve(name)`. Unknown template names degrade to L1
   with a WARN log — bad operator configuration cannot lose a ticket.
5. **L3 ships as a gate, not a merger.** The L3 auto-merge surface posts a
   `merge-proposed` or `merge-blocked` comment on the tracker; the actual
   PR-merge call is deferred. This separates "is this PR ready?" (a
   policy decision) from "perform a merge against GitHub's REST API" (an
   integration surface). A future ADR plumbs the merge call through.
   Readiness itself is now real: `TaskSource::check_merge_readiness` has
   a default `Blocked` impl for PR-less trackers (Linear) and a concrete
   GitHub implementation (`mergeable_state` + reviews via `octocrab`).
6. **External state changes reflect into the FSM.** When a tracker closes a
   ticket externally or adds `surge:disabled` mid-run, the `TaskRouter`
   forwards the event as `RouterOutput::ExternalUpdate`; the daemon calls
   `EngineFacade::stop_run` for an `Active` run and transitions the
   `ticket_index` row through the existing FSM (no new state variants).
   This closes the loop on "tracker is master": the user retains authority
   over the ticket lifecycle, and surge follows.

## Alternatives considered

**Per-tier columns on `ticket_index` (e.g. `tier: TEXT`).** Rejected because
labels can change after the row is written; re-reading labels at decision
points keeps the source of truth in one place (the tracker) and avoids stale-
state bugs. The cost is one extra `fetch_task` per RunFinished event for L3
gating — acceptable.

**Open trait for `AutomationPolicy`.** Rejected; a closed enum lets every
consumer match exhaustively (with `#[non_exhaustive]` for forward-compat)
and keeps the precedence table in one tested place.

**Inline the gate in `intake_completion`.** Rejected because it would mix two
concerns: the run-completion comment (which every tier needs) and the L3
auto-merge decision (only L3). Keeping them in separate consumers makes the
broadcast subscription contract explicit and lets the merge gate evolve
independently.

**Add a `TaskSource::merge_pr` method.** Rejected for this milestone — see
rationale #5. The decision surface lands first; the merge call lands in a
follow-up ADR with explicit per-provider semantics.

## Consequences

- Two new registry migrations: `0012_inbox_action_policy_hint.sql` adds the
  L2 carry column; `0013_intake_emit_log.sql` adds the per-side-effect
  dedup table.
- One new module per crate touched:
  - `surge-intake::policy` (resolver + label constants),
  - `surge-intake::cadence` (tier-aware polling algorithm; wiring deferred),
  - `surge-persistence::intake_emit_log` (idempotency log),
  - `surge-daemon::inbox::ticket_run_launcher` (shared launch helper),
  - `surge-daemon::automation_merge_gate` (L3 gate consumer).
- One new CLI subcommand: `surge intake list` (table + JSON outputs).
- `RouterOutput` becomes `#[non_exhaustive]` and gains `ExternalUpdate`.
- `TaskSource::check_merge_readiness` is part of the public trait, with
  a default `Blocked` impl so non-GitHub providers degrade gracefully.
  GitHub queries `mergeable_state` plus reviews; the actual
  `octocrab.pulls().merge()` call remains deferred per rationale #5.
- The `CadenceController` ships its algorithm + tests but is not yet wired
  into source poll loops. Doing so requires either a
  `TaskSource::set_poll_interval` method or a wrapping stream — both
  deferred per "decide or defer, never half-implement" discipline. See ADR
  0013 § "Tier-aware polling" for the staged plan (this same document).

## References

- ROADMAP § `Tracker automation tiers`
  ([`.ai-factory/ROADMAP.md`](../../.ai-factory/ROADMAP.md) lines 187-203).
- Plan: `.ai-factory/plans/tracker-automation-tiers.md`.
- Code: `surge_intake::policy::AutomationPolicy`,
  `surge_intake::cadence::CadenceController`,
  `surge_persistence::intake_emit_log::EmitEventKind`,
  `surge_daemon::inbox::ticket_run_launcher::TicketRunLauncher`,
  `surge_daemon::automation_merge_gate`.
