# Stage 1 — retained startup guard amendment

PROPOSED; owner verdict ACCEPTABLE pending focused critic/security review and
root release of the shape hold. No implementation authorization is issued here.
Initial stage 1 authoring is underway under its existing lease; feature repair
count remains0/3 shared, Home10 remains closed. Only this document is written.

Parent design remains accepted body f4cb0605fc46606d43c1d296cae75beb69cd618f2ede886c4d7eafb0c0db84c9.
Stage1 approved clarification is9edbc04e4309b3c3f59fa89287bed5174698daf8a9036d056d644a7ee7cf3006.
This changes only the owning storage of the already required constant startup
checker. All source-qualified ordering, authority exclusions, unavailable
semantics and later stages remain required. This is a shape amendment, not a new
feature or permission to weaken a guard.

## Confirmed failure and protected input

Actual partial source `execution_recovery/seed_guard.rs:38–58` reconstructs a
SeedStartupGuard from RunMemory only when a marker is about to be checked:
`executable = !stage_occurrences.is_empty()`, binding from work_item, graph hash
from verification_graph.identity(), with other fields reset to defaults.
Batch preflight and UI already retain/feed the shared guard through every event.

Root/author report authentic current-source oracle0/1 in
stage1/current-single-event-late-red.log: startup, OutcomeReported4 without a
StageEntered, then marker must report Late; incremental memory instead reports
Unavailable. The refusal is fail-closed and memory remains unchanged, but its
classification disagrees with the actual prefix and cannot support stage4's
seed-before-execution rule. The author owns this RED receipt; this task executes
no tests and does not claim a second observation.

Source also proves a second latent reconstruction hazard: full apply constructs
verification_graph using with_default_escalation_gates, while guard.observe
records PipelineMaterialized.graph_hash from the raw envelope. Rehashing the
normalized graph is not the raw registered startup identity. The author is
checking authentic ordinary marker controls on both paths before any fix; do not
claim this second behavior is reproduced until its receipt exists.

Protected invariant: marker classification uses the actual complete ordered raw
prefix, including events that do not modify lifecycle/semantic maps. Never infer
that prefix from a subset of derived memory. Raw run/binding/registered graph
metadata must survive once observed; defaults, graph normalization, snapshot
selection or a no-op must not reset it. Unavailable remains unavailable; no seed
read/execution authority is created by remembering structural metadata.

## Chosen minimal owning representation

Keep existing `RunMemory.semantic_lineage: Option<SeedLineage>` unchanged; all its
already migrated comparison consumers keep their API. Add one explicit fresh
36th field `semantic_startup_guard: SeedStartupGuard` using the EXACT existing
checker, not another phase enum or a new history framework. It is constant-size,
nonserializable structural observation, with private guard fields and no public
mutation that could bless a private seed. Remove from_memory/memory_guard history
reconstruction entirely. Do not keep a fallback reconstruction for old callers.

Full run-state application also needs the same retained checker before and after
Pipeline, where RunMemory does not exist. Retain it in the NotStarted,
Bootstrapping and Terminal variants; Pipeline uses memory.semantic_startup_guard.
A single private accessor and successful-result setter centralize storage access.
Use a default/not_started constructor at call sites rather than distributing
handwritten default guard literals. This is an explicit Rust API shape change:
NotStarted ceases to be a unit variant, and Bootstrapping/Terminal gain a field.
Existing enum patterns/fixtures must migrate and API review must account for it.
No persisted event/snapshot serialization changes follow: RunState/RunMemory are
not the private seed wire format and are not serialized as runtime authority.

Why include NotStarted/Terminal? Current apply explicitly accepts/pass-throughs
audit/no-op events in all states. A checker reconstructed as default at NotStarted
would lose an earlier such event; a terminal branch reset would likewise forget
the prefix. Retaining the same constant state preserves these existing lifecycle
behaviors while accurately tracking structural history. Do not fix this by
silently forbidding formerly accepted audit events, making terminal facts false,
or deriving a different guard from the terminal discriminant. Reusing a
RecoveryObserver field for seed metadata would obscure ownership and couple two
unrelated domains; a wrapper/second FSM is unnecessary.

A combined SemanticHistoryContext replacing the35th lineage slot would save one
RunMemory field but also require changing every new lineage consumer and does
not remove the need to retain the guard across non-Pipeline phases. The explicit
36th field is the smaller independent change and clearer exhaustive field policy.
Do not introduce a second optional seed identity or duplicate comparison context.

## Exact mutation protocol

Reuse SeedStartupGuard::observe/finish for every path, without changing the
classification table as part of this repair:

1. Single memory event: clone ONLY the constant retained guard to a candidate;
   CheckedMemoryEvent::prepare validates nonzero position and observes the real
   event on that candidate. finish checks availability. On any error, return
   before recovery/verification/ledger/cursor/guard mutation. On success, apply
   the checked event once and commit candidate guard to the same memory.
2. Atomic memory batch: clone that guard once, validate EVERY event into existing
   private checked inputs while advancing the candidate, then finish. Only after
   complete success apply the infallible checked mutations and assign final
   candidate guard. Earlier valid events in a failed batch change nothing.
3. Full-state apply: read the guard from the actual current variant, prepare one
   candidate/check before ANY recovery or verification work, perform the existing
   lifecycle transition on the owned state, and assign candidate guard to the
   successful returned variant through ONE final setter. Error returns no
   publishable new state. PipelineMaterialized must carry the candidate from
   Bootstrapping into the new memory; bootstrap approval/advance and terminal
   transitions must not reset it. Pass-through/no-op returns advance only this
   structural observation, not semantic maps/cursor/terminal kind/reason.
4. Full batch fold: existing complete-prefix validation still precedes publication;
   its local checker is the same algorithm, and repeated apply retains equivalent
   guard state. Do not double-feed the stored guard because full preflight also
   ran. A validation-only local guard does not mutate the per-state one.
5. UI/inspection: existing retained shared guards remain structural guards for
   their own separately owned projections. Do not inject their guard into engine
   memory or infer one from UI sessions. Incremental and replay guards use the
   same observe/finish classification, with current-generation publication and
   existing unavailable/decision suppression intact.

No whole RunMemory/graph/journal clone or rescan per event. Guard cloning copies
only constant metadata; observe uses raw registered graph_hash, not graph hashing
or profile normalization. No new launch/hydration authority, marker acceptance,
seed application or synthetic local sequence is enabled in stage1.

## Field policy and exact caller closure

RunMemory policy becomes36 fields:34 original fields; semantic_lineage fresh None
until validated seed normalization; semantic_startup_guard fresh default then
actual successor-local prefix observation. Never copy the source's guard into a
successor seed. Stage4 starts its own guard from its own RunStarted/binding/raw
registered graph and processes the actual seed marker; any hydrated extension
must retain this owner rather than rebuilding history from semantic maps.

Affected production closure, refreshed by exact rg inventory before editing:

- core run_state.rs: CheckedMemoryEvent preparation, memory apply_event/apply_events,
  full apply/fold, NotStarted creation/patterns, Bootstrapping creation/approval
  advancement, PipelineMaterialized, all terminal transitions and no-op arms;
  exhaustive36-field policy construction/destructuring.
- core execution_recovery/seed_guard.rs: remove from_memory and derived-graph
  hashing; keep the single existing incremental observer/classification API.
- core run_display.rs, run_report/mod.rs, stage_tool.rs and operator/pending/
  decisions.rs: RunState enum patterns/construction, typed errors; preserve real
  terminal completion and unavailable semantics. No new display policy.
- persistence runs/query.rs/inspection.rs; engine execution_state.rs and run_task
  route-preparation/apply_read_events: existing fallible propagation/atomic batch
  remains; no partial caller reset of semantic_startup_guard after successful
  application or selected checkpoint. Guards derive from complete raw events.
- UI run_stream.rs; daemon work_items.rs/intake_completion.rs; CLI mcp_serve.rs:
  RunState variants/constructors/patterns, existing shared guard and trust checks.
- affected core recovery tests/fold determinism/hook tests, daemon IPC fixture,
  engine initial-prompt tests and current stage1 semantic contract tests: explicit
  constructor/pattern updates without weakening no-op/terminal assertions.

`RunMemory::apply_event/apply_events` stay Result-returning as already approved;
private checked inputs remain private. This adds no persistence public API or
new event variant. The additional public RunState shape and RunMemory field are
reported honestly under the stage1 public API/semver gate, not called internal.

## Before-fix RED and required oracles

The author retains exact before-fix source and receipts, and first runs real
current-source assertions before guard-shape edits:

1. Confirmed OutcomeReported-without-StageEntered sequence: repeated memory
   application and repeated public full-state apply must return Late, matching
   batch preflight. Ensure normal prefix application succeeded first and rejected
   marker leaves caller memory/guard unchanged.
2. Ordinary valid startup marker controls through BOTH actual fold paths: recorded
   raw graph hash equals manifest despite normalized verification_graph differing;
   the marker returns Unavailable, never Binding. Capture before-fix discrepancy
   if present; do not describe a setup failure as RED.
3. RequirementRevised and HookExecuted/OutcomeRejectedByHook (no StageEntered),
   before/after PipelineMaterialized where the current state permits them: their
   guard effect survives even if semantic fields/lifecycle stay unchanged.
   Include no-op before RunStarted and after terminal, preserving existing
   accepted/no-op lifecycle behavior and never manufacturing a fresh startup.
4. Partition one fixed raw prefix at multiple boundaries: one batch, batches
   separated across marker checks, and one-event-at-a-time produce equal semantic
   memory AND retained guard on success, and the same typed marker error. Use
   explicit fixed IDs/outcomes/expected errors; no reimplementation oracle.
5. Failed batch has valid recovery/verification/ledger-changing prefix then zero,
   late/binding-invalid/unavailable marker; memory INCLUDING guard is unchanged.
   Subsequent legitimate application sees the last successful guard, not the
   failed candidate. No whole-memory clone in production; test clones are fine.
6. Bootstrap approval transitions→PipelineMaterialized→ordinary no-op→marker and
   Pipeline→true terminal→no-op preserve guard and terminal identity. Full-state
   apply and fold keep existing recovery exactly-once/verification behavior.
7. Existing UI incremental apply/apply_recorded and cold candidate tests remain;
   retain unavailable history/ownership/decision suppression. The source phase
   checker classifies the same fixed prefix without resetting on reconnect
   candidate replacement or publication, subject to existing Home ownership rules.

No marker succeeds with unhydrated stage1 data. A disagreement among Late,
Binding and Unavailable is not hidden by asserting only is_err(). Do not alter
existing all-or-nothing timestamp tests, current type zero REDs, source provenance
or authentic outer continuation EOF RED. Stage0 remains deliberately RED.

## Gates and disposition

After accepted focused reviews and root handoff, author runs affected complete
core/orchestrator/persistence/daemon/CLI/UI/notify closure, strict default and
all-feature Clippy, workspace fmt/diff checks, current compatibility/serde/schema
oracles and required API review. Inventory every RunState::NotStarted and newly
extended variant constructor/pattern, including integration/external consumers;
no broad wildcard introduced to hide missing semantics. Stage1 full5a and required
5b remain, with exact source freeze/receipts and unchanged outer RED declared.

Owner verdict ACCEPTABLE: this is the minimal coherent retained owner for the
already accepted shared guard. The36th-field and public variant-shape amendment
is explicit and requires independent focused review before changing source.
No product fork or new strategic scope; private seeds, workspace ownership,
continuation endpoint, native app journey and platform evidence remain later.

## Actual amendment handoff

Independent critic and security reviewers both returned ACCEPTABLE for body SHA
`0a52fd0f1fd130695501321309df8905b0456fdc0a84a2d7ba502ea9a7517c79`.
Exact reviewed bytes and verdicts are retained in stage1/approved-guard-amendment-snapshot.md
and stage1/guard-amendment-plan-review.json. Root released the shape hold to the
sole builder for initial Stage1 implementation. Shared feature repair count stays0/3;
this is design refinement before final5a/5b, not feature completion.
