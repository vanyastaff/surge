---
title: "feat: MCP cold recovery by group cleanup and restart (ADR-0021)"
type: feat
status: completed
date: 2026-10-05
---

# feat: MCP cold recovery by group cleanup and restart (ADR-0021)

## Summary

Implement [ADR-0021](../adr/0021-mcp-restart-recovery.md): a run whose prior
host-launched MCP writer died with the daemon resumes automatically after the
writer's process group is stopped and a fresh server is started from the frozen
manifest. Interrupted tool calls are never replayed; the agent is told about
them. Fail-closed refusals for wrong identity or wrong inputs stay. This
replaces the managed VM + broker path of ADR-0020 and unblocks the macOS
release once the rewritten acceptance passes.

## Problem frame

- `mcp_cleanup_refusal` (`crates/surge-orchestrator/src/engine/writer_coverage.rs`)
  refuses every cold resume with a prior MCP writer: MCP writers are registered
  with `local_effects = false`, so even a probed-`Gone` `GroupOnly` group is
  "unconfirmed descendant or external-effect cleanup".
- Park and suspension only count as clean with `fence.cleanup_confirmed`, which
  native MCP can never produce. The populated acceptance fixture waits for it
  and times out (`initial_mcp_evidence_ready`,
  `crates/surge-daemon/tests/owned_flow_mcp_recovery.rs`).
- MCP children already run in their own process group (`process_group(0)` in
  `crates/surge-mcp/src/connection.rs`) and record `WriterCoverage::GroupOnly`
  with a boot-session-aware leader identity. The missing pieces are group
  termination on recovery, a named best-effort cleanup record, restart from the
  frozen manifest, and the interrupted-call notice.

## Scope

In: host-launched stdio MCP writers on macOS and Linux; daemon cold recovery
and park/wake; run events and their folds; the agent resume context; the ten
original acceptance cases.

Out: VM or cgroup containment; an external-effect broker; Windows (deferred by
the macOS scope); MCP servers launched by an ACP runtime from its own config.

## Design

1. **Cleanup record.** Add an explicit best-effort cleanup outcome for an
   execution writer, for example `ExecutionWriterCleanup { writer, basis:
   GroupStopped { coverage: GroupOnly } }`. It is distinct from
   `cleanup_confirmed`, which keeps its current meaning. The fold exposes both;
   reports and UI say "process group stopped". The schema change follows
   [schema versioning](../schema-versioning.md) with a migration test.
2. **Group termination.** In the recovery path, for each MCP writer without a
   cleanup record: validate the container identity; if the group is live, send
   TERM to the group, wait for a bounded grace, send KILL, probe again. Reuse
   `surge_acp::process_evidence::{observe, probe}`. Never signal a group whose
   leader identity does not match (PID reuse protection stays).
3. **Refusal policy.** `mcp_cleanup_refusal` refuses only on conflicting
   ownership evidence, missing process identity, or a group still live after
   termination. A `GroupOnly` writer with a best-effort cleanup record passes.
   The suspension fence for park/wake accepts the same record.
4. **Restart.** The resumed stage starts MCP servers from the frozen accepted
   manifest exactly as on first start; manifest or private-input mismatch keeps
   the existing refusal and Inbox attention item.
5. **Interrupted calls.** At recovery, find MCP tool calls with an intent and no
   result in the run log and record an outcome-unknown event for each. Bind a
   short notice into the resumed agent prompt: server, tool, time, "outcome
   unknown, check before retrying". No automatic replay; no human gate by
   default.
6. **Observability.** Tracing spans for termination (writer, group, signals sent,
   grace used, final liveness) and the restart; no private manifest material in
   logs, preserving the existing opacity assertions.

## Acceptance mapping — the ten original cases

Eight cases fail in fixture setup today: `start_park(…, "populated")` waits for
`RunSuspended { fence.cleanup_confirmed }`. Their own assertions are
fail-closed or transport/opacity oracles that ADR-0021 keeps. Each row is
confirmed during implementation; any change beyond this table needs a recorded
reason.

| Case | Today | Under ADR-0021 |
|---|---|---|
| `legacy_global_policy_references_survive_actual_start_and_resume` | resume refused, unconfirmed cleanup | Unchanged assertions; passes via restart |
| `optional_whitelists_and_server_override_survive_actual_start_and_resume` | resume refused, unconfirmed cleanup | Unchanged assertions; passes via restart |
| `populated_initial_transport_is_exact_and_public_snapshot_opaque` | setup timeout | Setup waits for the best-effort record; opacity oracle unchanged |
| `populated_cold_valid_twin_keeps_original_transport_and_writer_history` | setup timeout | Transport and history oracles unchanged; "production-owned cleanup evidence" becomes the best-effort `GroupStopped` record |
| `ordinary_accepted_manifest_updates_preserve_exact_first_snapshot` | setup timeout | Setup change only; manifest-update rejection unchanged |
| `missing_and_corrupt_private_inputs_refuse_real_cold_writers` | setup timeout | Setup change only; refusal before any new effect unchanged |
| `transplanted_objects_and_changed_manifest_refuse_real_cold_writers` | setup timeout | Setup change only; refusal unchanged |
| `manifest_binding_refusal_must_persist_attention_after_actual_wake` | setup timeout | Setup change only; persisted attention unchanged |
| `published_refusal_retains_original_os_lease_without_caller_guard_until_real_delivery` | setup timeout | Setup change only; lease retention unchanged |
| `cold_refusal_reclaims_actual_event_before_ack_without_new_claim_or_effect` | setup timeout | Setup change only; reclaim-without-new-effect unchanged |

New cases:

- a live group (server that ignores TERM) is killed, recorded and restarted;
- a group that survives KILL (simulated through the probe seam) refuses with attention;
- a mismatched leader identity is never signalled;
- an interrupted `tools/call` is recorded as outcome-unknown, not replayed, and
  appears in the resumed prompt;
- reports and the desktop never render the best-effort record as "confirmed".

## Implementation status — 2026-10-05

Done: `surge_acp::process_evidence::{group_state, stop_group}`, the
`ExecutionWriterGroupStopped` event (schema v17) with its fold, and
`writer_coverage::{assess_mcp_cleanup, stop_and_assess_mcp_cleanup,
record_mcp_groups_stopped}` wired into the park check, the resume check and
the resumed run's start. The record is written on resume, not at park:
appending before `seal_suspension` would move the log past the fence's
snapshot. Owned-flow MCP suite 23/23 locally.

Done (step 5): `ToolCalled` is appended before dispatch, `RunMemory.unresolved_mcp_calls`
folds MCP calls without a result and clears on the next `SessionOpened`, and the
first stage after an interruption gets an "Interrupted tool calls" notice before
its prompt. No new event or schema bump was needed.

Oracle changes beyond the mapping table, with reasons:

- `populated_cold_valid_twin_keeps_original_transport_and_writer_history` now
  requires a cleanup record (closure or group stopped) for writers before the
  suspension. The resumed phase's own writers end with the run, and no record
  is appended after `RunCompleted`.
- `unresolved_mcp_domain_requires_attention_and_blocks_actual_cold_dispatch`
  keeps every assertion. Its fixture now leaves a descendant in the MCP
  child's process group, which is the case ADR-0021 still refuses; an empty
  group is no longer an unresolved domain.

## Verification

Passed on CI run 37391960503 (`3e5db7d`): Test Suite and Clippy green on
macOS and Ubuntu.


`cargo clippy --workspace --all-targets --all-features -D warnings`; nextest
for surge-core, surge-persistence, surge-orchestrator, surge-mcp and
surge-daemon (actual mock agent prepared); the full owned-flow MCP suite at
23/23 on macOS and Linux CI. Use `CARGO_INCREMENTAL=0`; disk is tight.

## Risks

- Folding a new cleanup basis into existing fences can accidentally relax a
  refusal that should stay. Mitigation: the refusal cases above keep their
  assertions byte-for-byte.
- An agent may still repeat a non-idempotent call despite the notice
  (accepted in ADR-0021).
