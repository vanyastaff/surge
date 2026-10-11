# Role 8 — backend release inspection

Read-only inspection of daemon/orchestrator integration. Requested crate CLAUDE.md files do not exist. No source mutations, no cargo invocation, no external effects.

## Concrete finding: P1 — L3 automation never recovers a dropped completion

Evidence: crates/surge-daemon/src/automation_merge_gate.rs:82–122 has a receive-only broadcast loop. Lagged at line 97 logs dropped RunFinished events and immediately continues. There is no startup sweep, periodic sweep, journal scan, or durable work queue. In contrast intake_completion.rs:59/69/86 implements all three reconciliation triggers and transitions the ticket from its existing terminal journal without broadcasting a replacement RunFinished event.

Minimal reproduction to add: register an L3 mock ticket with a durable Completed run journal; start the merge-gate receiver after its completion was emitted (or force lag with a tiny broadcast channel); wait for an explicit reconciliation tick. Expected: readiness checked and merge or blocked escalation delivered. Current control flow: no handler invocation ever occurs unless a fresh unrelated future completion for the same run is externally re-emitted.

A hung tracker request is a further way to lose subsequent events: fetch/readiness/merge/comment awaits run serially in the receiver without a timeout. The primary defect remains missing recovery of lost delivery.

Minimal fix plan: query correlated terminal Completed runs with no terminal merge decision; include terminal tickets, since the completion consumer may already settle them. Reuse intake_completion::durable_outcome, invoke existing handle_completion so current labels and head readiness remain checked, and run reconciliation at startup, periodically, and on lag. Keep each pass bounded/paged; do not add any blanket fixed LIMIT that permanently starves older candidates. Add mock-based startup/loss tests and suppression after durable Merged decision. Outward effects use existing policy only. Own automation_merge_gate.rs; any persistence query addition needs sole owner coordination.

Checks actually run: source inspection via rg/sed, Git recent WIP stat inspection; Python structural assertions PASS verifying receive-only/no-reconcile loop and three completion-consumer reconciliation call sites. No Rust runtime verification performed; finding is source-proven, reproduction test not yet run. Existing initial search found no todo!/unimplemented!/TODO/FIXME under daemon/orchestrator src; that does not certify completion.

Release implication: when shipping advertised L3 automated merge/recovery, lost completions silently strand its core outcome. Treat as release blocker until reproduced and fixed or explicitly documented outside the release scope.
