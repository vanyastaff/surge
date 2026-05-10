---
status: accepted
deciders: vanyastaff
date: 2026-05-07
supersedes: none
---

# ADR 0004 — Bootstrap HumanGate semantics

## Context

The M6 `HumanGate` stage handler (`crates/surge-orchestrator/src/engine/stage/human_gate.rs`) supports a generic `approve` / `reject` shape via `ApprovalOption[]` configured per gate. Bootstrap (Description / Roadmap / Flow) needs a **third** outcome — `edit` — and a structured edit-loop with a configurable cap, so an operator can iterate with the agent until satisfied.

We could either:

1. Push these semantics into every consumer of `HumanGateConfig` (each bootstrap profile redeclares the three options and the engine pattern-matches on profile name), or
2. Add a typed mode to `HumanGateConfig` that the engine recognizes once, centrally.

## Decision

We add `mode: HumanGateMode` to `HumanGateConfig` with `#[serde(default)]` and the following shape:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanGateMode {
    #[default]
    Generic,
    Bootstrap { stage: BootstrapStage },
}
```

Existing TOML round-trips unchanged (the field is optional and defaults to `Generic`).

In `Bootstrap` mode the gate handler:

1. Emits `BootstrapApprovalRequested { stage, channel }` immediately before the existing `HumanInputRequested`.
2. Accepts three outcomes — `approve`, `edit`, `reject` — regardless of how `options` are populated. Operator-supplied free-text feedback (the `comment` field already on the operator's response payload, see `crates/surge-core/src/run_event.rs` `HumanInputResolved.response`) is required when the outcome is `edit`.
3. Emits `BootstrapApprovalDecided { stage, decision, comment }` after `HumanInputResolved`.
4. On `edit`: emits `BootstrapEditRequested { stage, feedback }` and routes via the `Backtrack` edge that the bootstrap graph wires from this gate back to its preceding Agent node. Routing semantics for `EdgeKind::Backtrack` are wired in Task 27 of the implementation plan.
5. On `approve`: emits `OutcomeReported { outcome: "approve" }` and falls through to the gate's forward edge as today.
6. On `reject`: emits `OutcomeReported { outcome: "reject" }` and routes to terminal (existing `HumanGateRejected` semantics).

## Edit-Loop Cap

`EngineRunConfig.bootstrap.edit_loop_cap` (new field, default `3`) caps the number of `edit` round-trips the gate accepts for a given stage. The counter is per-stage and persisted deterministically:

- New `RunMemory` field: `bootstrap_edit_counts: BTreeMap<BootstrapStage, u32>`.
- Fold rule (`crates/surge-core/src/run_state.rs`): on each `BootstrapEditRequested { stage, .. }`, increment `bootstrap_edit_counts[stage]`. Folds remain deterministic and do not read the wall clock or generate IDs.

When the gate handler enters and observes `bootstrap_edit_counts[stage] >= edit_loop_cap`, it returns `StageError::EditLoopCapExceeded(stage)` instead of emitting `BootstrapEditRequested`. The engine surfaces the error as a terminal failure with an explicit `EscalationRequested` notify event via the existing `surge-notify` channels.

The cap counts **edit decisions**, not gate visits. A cap of `3` allows up to three rounds of operator-driven edits before forced escalation.

## Operator Response Schema

In Bootstrap mode the gate's `HumanInputRequested.schema` advertises:

```json
{
  "type": "object",
  "properties": {
    "outcome": { "type": "string", "enum": ["approve", "edit", "reject"] },
    "comment": { "type": "string" }
  },
  "required": ["outcome"]
}
```

When `outcome == "edit"`, `comment` is required and becomes the `feedback` field on the emitted `BootstrapEditRequested` event. The agent receiving the re-entry binding sees this string via `ArtifactSource::EditFeedback { from_node }` (Task 6 + Task 8 in the plan).

## Composition With Existing M6 HumanGate

`HumanGateMode::Generic` is the default and behaves exactly as today: no bootstrap-specific events emitted, no special outcome enforcement, `options` define the allowed outcomes. Existing `HumanGate` consumers (e.g., milestone-progression gates inside the materialized pipeline) need no change.

## Consequences

**Preserves:**

- Closed-enum invariant on `BootstrapStage` and `BootstrapDecision` — both already declared in `crates/surge-core/src/run_event.rs:348-362`. No new variants needed.
- Determinism of `fold` — the new `bootstrap_edit_counts` field is a `BTreeMap` and the fold rule is a pure increment.
- The `surge-core` I/O-free leaf invariant — all storage and channel side-effects stay in the orchestrator.

**Enables:**

- Bootstrap UI surfaces (CLI today, Telegram cards in a future milestone) implement a single contract — three buttons (`approve`/`edit`/`reject`) with an optional free-text field — for **all** bootstrap stages.
- Telemetry (Task 24) observes `bootstrap_edit_counts` directly.

**Forces:**

- The bootstrap graph (`crates/surge-core/bundled/flows/bootstrap-1.0.toml`) MUST wire `Backtrack` edges from each bootstrap HumanGate back to its preceding Agent. The validator (Task 28) accepts these cycles because at least one edge in each cycle is `Backtrack`.

## Out of Scope

- Approval channels other than the M5 console fallback (`surge-notify` Telegram cards land in their own milestone).
- Per-stage cap overrides (today: one global cap on `EngineRunConfig`).
- Resuming a paused edit-loop after a daemon restart (handled by the future `Crash recovery` milestone).
