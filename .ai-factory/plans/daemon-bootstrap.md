# Durable daemon bootstrap — proposed implementation plan

Status: **DESIGN ACCEPTED — independent adversarial and domain review passed.**
Implementation slice 1 (durable repository and disabled API) passed independent
spec and code-quality review plus local affected-crate gates. Supervisor slices 2–3
are implemented in the working tree with focused tests green; broad checks and
independent acceptance remain pending. Production wiring now supplies the owner,
while desktop lineage and pending-input recovery (slice 4) remain unimplemented.

## Scope and ownership

Implement an explicit daemon-owned operation for desktop prompt → isolated
planning → approved implementation. Reuse the existing engine, admission
controller, Git manager, event storage, and bootstrap materialization helper.
Do not create a generic jobs subsystem or recognize bootstrap intent by graph name.

The first supported input is an existing, configured Git repository with a commit
and clean tracked/untracked state. New-application initialization, dirty overlays,
and arbitrary per-project runtime configuration remain open product requirements.
Legacy `StartRun` and its RAM-only `PendingStarts` route remain unchanged and are
not described as durable by this feature.

Daemon owns orchestration and admission; persistence owns transactions and storage;
Git owns OID-based worktree preparation; orchestrator owns run execution and
materialization; desktop owns presentation and submission/reconnection behavior.

## Existing reusable surfaces and gaps

- `bootstrap_driver::materialized_run_from_completed` extracts the generated graph
  and required description, roadmap, and flow artifact references. It does not
  establish durable success, validate artifact bytes/digests, or perform all graph
  validation required for continuation.
- `EngineRunConfig.bootstrap_parent` copies these artifacts into a child. It does
  not persist a reliable parent/child relationship.
- `AdmissionController` remains the active-slot scheduler. Its queue and the
  server's pending-start map are currently in memory.
- `GitManager::create_run_worktree` creates managed branches/worktrees, but resolves
  a local branch at creation time. It needs a narrow frozen-commit-OID path.
- `RunStarted.project_path` contains the execution worktree path. Existing recovery
  nevertheless guesses `<worktrees_root>/<full-run-id>`; bootstrap-owned runs must
  use their saved paths and must not also enter that legacy recovery path.
- The daemon constructs one engine with profile registry, agent registry, and
  capacity policy loaded at startup. Saving a project config does not override
  these engine-level objects.
- Persistence subscriptions replay history from sequence zero. Reconnection must
  reconstruct pending requests and deduplicate live delivery by sequence; it must
  never answer historical requests as current approvals.

## Explicit API and rollout boundary

Add `StartBootstrap`, a daemon-client method, and operation status/cancellation
surfaces. Keep ordinary `StartRun` semantics separate. A client supplies a stable
operation ID and prompt/project intent, not an arbitrary serialized engine config.

Slice 1 builds the repository and request/status contract, but **must not enable
production acceptance until a supervisor can execute and recover accepted work**.
Before that prerequisite exists, the production request returns an explicit
unsupported/not-ready error without inserting an accepted operation. Repository
tests may exercise durable records directly. The UI remains on its existing
honestly labeled planning path until execution and recovery gates pass.

Once enabled, acknowledgement means the durable operation was accepted/queued,
not that planning started or implementation finished. A cancellation acknowledgement
means cancellation intent was durably recorded; completed cancellation is a
separate observed status.

## Versioned durable payload

Add a bootstrap-specific registry table/repository (`bootstrap_operations`), not a
generic executable job table. Persist an allowlisted daemon-owned payload with an
explicit schema version. Persistence must not depend on orchestrator Rust types.

Immutable fields:

- client operation ID and canonical client-intent fingerprint;
- reserved planning and implementation `RunId`s, generated once;
- canonical source-repository path and full frozen commit OID;
- exact planning/implementation worktree paths and expected Git branch identities;
- exact prompt;
- bootstrap template/graph reference and digest;
- captured project-context artifact reference and digest;
- effective runtime/profile/config references and semantic fingerprints;
- operation budget policy and limits.

Mutable fields include queue sequence, phase, revision, monotonic cancellation
intent, active run identity, parking information, child launch data, and typed
result/attention reason. An attention record preserves its blocked phase and run
identity. Child launch data is immutable once committed.

Do not persist arbitrary `EngineRunConfig`, `SurgeConfig`, MCP environment maps,
credential values, or `memory_store_path`. Authentication remains in installed
agent authentication, existing credential stores, or explicitly named environment
references. Required missing credentials produce attention without exposing values.
Version 1 rejects unsupported raw MCP/environment overrides instead of dropping
them. Hash credential references, not credential values; credential rotation is not
a configuration mismatch. Avoid writing secrets into diagnostics or fingerprints'
human-readable inputs.

Canonical client-intent serialization must define field order, defaults, lexical
absolute-path normalization, and fingerprint version without consulting mutable
repository/configuration state. The fingerprint covers only client-supplied
intent, excluding captured HEAD, config/context/runtime digests, and other
server-captured facts. Store those captured digests separately.

Look up the operation ID and compare client intent **before** recapturing HEAD,
loading configuration, checking repository cleanliness, or resolving current
filesystem identity. An identical retry returns the existing record even when
HEAD/configuration changed or the operation completed/was cancelled. A different
intent with the same ID rejects without mutation. Only a genuinely new ID performs
environment capture and preflight. Unknown stored payload versions remain inspectable but
cannot execute; do not deserialize them as the latest version with guessed defaults.

## Runtime identity and actual configuration use

Construct an immutable `DaemonRuntimeIdentity` from the same resolved objects
passed into the engine: effective profile registry, agent registry, and capacity
policy. The selected project's required effective settings must match that
runtime; otherwise reject with explicit configuration/restart guidance.

Persist references plus semantic fingerprints, not complete registry bodies.
Before initial planning start, implementation start, and either resume, resolve
and compare those references against the engine's actual immutable runtime identity.
Missing or changed referenced content transitions to `NeedsAttention` and launches
nothing. Reading fresh files alone is insufficient if the engine still holds older
objects. Use the verified objects for graph/profile resolution and capture the
context bytes referenced by the accepted request.

Version 1 does not introduce per-operation engines or claim that existing
`EngineRunConfig` overrides engine-global registries. Projects requiring different
effective runtime settings are unsupported until a later owner change. Hash-pinned
references intentionally trade automatic progress after configuration changes for
reproducible, reviewable execution.

## State machine and transaction boundaries

Use typed phases:

| Phase | Meaning |
|---|---|
| `QueuedPlanning` | Durable acceptance; planning awaits admission. |
| `PreparingPlanning` | Admission claimed; reconcile isolation and planning start. |
| `Planning` | Reserved planning run has valid startup evidence. |
| `QueuedImplementation` | Parent success verified; child launch data committed. |
| `PreparingImplementation` | Child admission claimed; reconcile child isolation/start. |
| `Implementing` | Reserved child has valid startup evidence. |
| `Cancelling` | Durable cancellation requested; settle any active/preparing run. |
| `Completed` | Durable child success observed. |
| `Failed` | Durable parent/child failure or definite unrecoverable operation failure. |
| `Cancelled` | No launch remains possible and any active run has settled as cancelled. |
| `NeedsAttention` | Execution/recovery cannot be confirmed safely; preserve blocked phase, run identity, and typed reason. |

Parking retains the phase and active run, plus recorded wake time. It is not a
terminal failure or permission to launch a child. An attention state never clears
cancellation intent. Terminal/attention results carry typed reasons rather than
message-string parsing.

Version 1 resumes `NeedsAttention` only through an explicit operator retry after
the original pinned inputs have been restored and all blocked-phase checks pass.
Background reconciliation must not silently restart it. Changed intent requires a
new operation ID; retry never refreshes pinned data. Cancellation of an already
terminal operation returns that existing terminal state without relabeling it.

Transactions and ordering:

1. **Acceptance:** first look up the ID and compare client intent as above. For a
   new ID, validate input/support, capture immutable data, then insert IDs,
   payload, separate intent/capture fingerprints and queue sequence in one
   transaction. A concurrent insert resolves through the same intent comparison,
   never an overwrite. Reply after commit.
2. **Admission claim:** compare phase/revision and `cancel_requested = false`, then
   claim preparation/start. Existing admission accounts for the slot. Release any
   provisional slot when the compare-and-swap loses or persistence fails. Queue
   order comes from persisted sequence and is rebuilt on daemon restart.
3. **Cancellation:** atomically set the monotonic cancellation flag, increment
   revision, and prevent future admission/continuation claims before acknowledging.
4. **Continuation:** after authoritative durable parent success, independently
   validate the materialized graph and artifact bytes/digests (see below),
   calculate the child budget, and atomically commit child launch data plus
   `QueuedImplementation`, conditional on cancellation still being false.
5. **Terminal:** record the typed durable child outcome, or the parent failure/
   abort when implementation was never eligible. Failure to persist is not success.

Use a daemon-local per-operation guard to serialize side effects. Never hold a
SQLite transaction across Git, IPC, engine execution, or other awaits. Cancellation
may still be committed concurrently with preparation: the worker checks it after
preparation and after obtaining the engine handle. If admission/start won first,
stop and join the reserved run. A racing cancel response does not promise that an
already-authorized start never happened; it promises durable intent and eventual
settlement or explicit attention.

Release the parent's admission slot before queuing/admitting implementation;
`max_active = 1` must progress. Startup recovery, periodic reconciliation, IPC, and
wake scheduling share the same operation ownership and tracked admission path.
Broadcast notifications only trigger reconciliation; durable records/events decide
what happens. Exclude operation-owned IDs from independent legacy recovery/wake
claims, or route those claims through this same owner.

## Git isolation and reconciliation

Capture the full commit OID while accepting a clean source repository. Add a
narrow OID-based managed-worktree preparation/reconciliation helper in `surge-git`.
Both planning and implementation start from that accepted OID, each in its own
worktree. The child inherits approved planning artifacts through existing artifact
seeding; it never copies arbitrary modifications from the planning worktree.

Persist the expected paths and branch identities before Git mutation. Reconcile:

- No branch/worktree: create from the recorded OID.
- Expected branch exists, worktree absent: finish registration only after checking
  repository, branch/OID, path, and recorded ownership. Never reset a conflicting
  branch to make the operation fit.
- Matching registered worktree: reuse.
- Foreign content, conflicting registration/branch/path, or missing required base:
  attention; no overwrite, deletion, or execution in the source checkout.
- Previously executing worktree: verify ownership/identity without requiring a
  clean tree or unchanged HEAD. Legitimate execution can edit and commit work.

Preserve failed worktrees and diagnostics. Worktree existence alone never proves
successful preparation. Empty repositories and dirty source inputs return clear
unsupported errors; their full product workflows remain open.

## Partial engine startup and typed outcomes

Engine startup creates storage before persisting its initial event batch. Reconcile
distinct facts rather than treating any registry row as a started run:

- No run storage/start evidence: eligible recorded intent can attempt the same
  reserved ID, subject to current phase/cancellation/runtime checks.
- Valid `RunStarted` plus initial graph/config: resume that same ID, or inspect its
  durable terminal outcome. Do not create a replacement ID.
- Database/registry artifacts with incomplete or contradictory startup evidence:
  version 1 enters attention. Do not delete partial data and blindly retry.
- Active in this daemon: attach/reconcile status, never start again.

Planning continuation requires typed durable `Completed`; legacy recovery's
`failed: bool` is insufficient. `Failed`/`Aborted` forbids child launch, `Parked`
remains pending, and a missing/ambiguous outcome requires attention. A completion
handle is joined before normal continuation, and durable events reconcile a crash
between completion and supervisor updates. Parent planning completion must never be
presented as implementation completion.

The continuation owner must perform explicit validation before committing child
intent: confirm typed durable parent success, read the required artifact contents
through the existing artifact-store boundary, verify their recorded digests and
applicable artifact contracts, and run the execution graph's full validation.
Confirm that the graph/reference set corresponds to the approved materialization,
then pin those validated identities in child launch data. Reusing
`materialized_run_from_completed` supplies the extraction step only; its success
must not stand in for these checks. Missing, mutated, corrupt, or invalid content
blocks continuation before child intent/start.

## Shared operation budget

Persist the operation budget once. Planning uses that allowance. Before child
intent commits, fold durable planning usage and persist the child's remaining
positive allowance; restart must reuse that saved allowance, not reset the budget.
If any enforced dimension is exhausted, do not launch implementation. Existing
`BudgetLimits` treats zero/nonpositive values as unlimited, so never encode an
exhausted remainder as a zero engine limit.

Current engine enforcement happens at stage boundaries. Do not advertise a strict
spending ceiling. Known costs and tokens can use existing budget plumbing with a
remaining allowance. Unknown cost is not known zero. If a requested hard USD limit
cannot be enforced from available price/usage evidence, reject it as unsupported at
acceptance or enter attention before further execution when the gap is discovered.
Do not silently downgrade an abort policy to warnings. Supporting precise hard
limits and missing-usage reconciliation remains an explicit requirement.

## Dependency slices and observable acceptance

### 1. Durable API/repository (production acceptance remains disabled)

Files: registry migration, focused persistence repository/export, typed versioned
daemon payload, IPC/client request/status/cancel contracts.

Red-first tests with real SQLite:

- committed acceptance and lost reply; identical retry returns identical IDs;
- lost-reply retry after HEAD/config changes or a dirty source still returns the
  original record before any environment preflight; captured pins stay unchanged;
- conflicting retry cannot alter prompt/config/IDs, including after terminal state;
- supported/unknown payload versions and invalid fingerprints;
- no raw credential/env/test-path fields in persisted payload or diagnostics;
- cancellation commit failure cannot be acknowledged as accepted cancellation;
- monotonic cancellation versus admission claim;
- OPEN, deferred to slice 3 with scope approval: child-intent transaction and its cancellation race. Slice 1 does not implement or claim this continuation boundary;
- cancelling an already completed/failed/cancelled operation returns its existing
  terminal state and cannot relabel it;
- queue sequence survives storage close/reopen;
- production API rejects unsupported/not-ready execution until supervisor enabled.

### 2. Isolated planning execution and recovery

Preproduction prerequisite: extend the v1 capture with the canonical Git common
directory identity from `PinnedRunBase`, alongside repository path and commit OID.
Slice 1's path-only capture must not be enabled for execution. A repository
replaced at the same path after restart must fail identity reconciliation.

Files: `surge-git` OID helper, daemon bootstrap supervisor, tracked-start seam,
daemon startup/recovery/wake ownership, runtime identity construction.

Acceptance cases:

- dirty/empty/misconfigured source rejects without accepted executable work;
- source HEAD moves after acceptance; planning still starts at recorded OID;
- interruption before branch creation, after branch creation, and after worktree
  registration; no duplicate worktree or overwrite;
- foreign path/branch conflict and missing base → attention;
- runtime/profile/config mismatch or missing credential → attention, no start;
- attention retains blocked phase/run/reason; restoring pinned inputs alone does
  not trigger background restart; explicit retry revalidates the original pins;
- changed intent cannot resume an attention record and requires a new ID;
- supported settings reach actual engine objects, not only persisted metadata;
- no storage, partial startup, valid startup, active run, and terminal run are
  handled distinctly;
- restart preserves queue order and exact paths; legacy recovery cannot claim the
  same owned run;
- cancellation during queueing/preparation/start settles or reports unconfirmed
  state; no confirmed-cancelled record later resurrects;
- no operation executes in the source checkout.

### 3. Durable bootstrap continuation

Files: daemon bootstrap supervisor, existing materialization helper integration,
child artifact seeding, shared remaining-budget calculation.

Acceptance cases:

- crash after parent completion but before child-intent commit;
- crash after child-intent commit but before child start;
- child startup persisted but supervisor update lost; resume same child ID;
- duplicate broadcasts/reconciliation produce one child and one durable linkage;
- `max_active = 1`, queue saturation, and parking/wake remain correct;
- cancellation races parent completion and child admission/start;
- parent failure/abort never launches child; parent success is not operation success;
- missing/corrupt artifacts or invalid generated graph → attention/failure;
- helper extraction success cannot authorize continuation with a non-successful
  durable parent, mutated artifact bytes, digest mismatch, invalid artifact
  contract, or graph validation failure; no child intent is committed in any case;
- child starts at frozen base and receives required artifacts without copying
  arbitrary planning-worktree edits;
- planning usage reduces child allowance; zero remainder does not become unlimited;
- unknown-cost hard limit cannot silently pass; restart cannot reset spending;
- storage failures cannot fabricate durable completion or cancellation.

### 4. Desktop lineage and pending-input recovery

Files: desktop dispatch/status/run-stream presentation, narrow daemon snapshot or
history surface as required by the existing pending-input fold.

Acceptance cases:

- select the accepted operation and its actual current parent/child run;
- separate queued/planning/implementing/attention/completed statuses;
- reconnect after a gate request restores only currently pending requests;
- snapshot/live sequence boundary neither loses nor duplicates events;
- an old decision ID cannot answer a later gate;
- implementation result and review evidence survive desktop/daemon restart.

## Verification and delivery gates

For each slice: external observable test first (RED), implementation (GREEN),
affected-crate strict clippy/tests and workspace format checks, independent spec
compliance then quality review. Integration fixtures use real SQLite, temporary
Git repositories, and controlled ACP doubles. Recovery tests reopen storage and
reconstruct the supervisor; calling reconciliation twice in one live process is
not restart evidence. Add explicit crash/failure seams at the listed boundaries.

No UI switch to `StartBootstrap` before planning execution, durable continuation,
and relevant recovery gates pass. No claim of the complete prompt-to-app journey
until reconnect/approval handling and an actual supported-runtime run are verified.

Open requirements are not retired by this narrower implementation: new-app init,
dirty-source overlays, arbitrary project runtime contexts, complete credential/MCP
reference support, precise hard budget enforcement, and legacy queue durability.


### Slice 1 review record

Spec repair dispatch 1/3: cancellation settlement may require `NeedsAttention`
while `cancel_requested` remains true. That state must preserve blocked phase,
reserved run, and typed reason across reopen; it must never become executable
through queue enumeration, claim, or retry. Terminal settlement reason expansion
remains a prerequisite before activating continuation in slice 3.

### Supervisor integration brief (pre-code reviewed)

The next slice reuses the completed Git prerequisite and the separately owned
read-only `Storage::inspect_run` / daemon `classify_startup` prerequisite. It does
not activate production submission until continuation can consume a successful
planning run. This is implementation sequencing, not completed product behavior.

1. **Runtime capture:** add daemon `bootstrap_runtime.rs`; retain clones of the
   exact profile registry, agent registry, and capacity objects that `main.rs`
   passes into `Engine::new_full`. The immutable runtime snapshot resolves the
   bootstrap graph and referenced profiles through those objects. New intent uses
   `SurgeConfig::discover_from`, `GitManager::capture_clean_base`, bundled bootstrap
   graph resolution, and project-context loading. Pin canonical Git common-dir,
   repository, commit OID, managed run paths, content references and hashes. Project
   requirements must match the actual engine snapshot; failed/defaulted daemon
   configuration is an explicit bootstrap preflight failure. Environment resolution
   and Git operations run outside database transactions; journal fields remain
   allowlisted references/digests, not raw runtime configuration.
2. **Supervisor ownership:** add a focused daemon supervisor taking the existing
   store, storage, facade, runtime snapshot, admission controller, broadcast
   registry, and shutdown token. A per-operation async guard owns side effects.
   Enumerate durable queued records in sequence order, provision a slot using
   `try_admit_no_queue`, then claim the phase by revision CAS. Never enqueue these
   IDs in the legacy RAM queue. Release the slot on a lost CAS or storage error.
   Reconcile `RunWorktreeSpec` from captured `PinnedRunBase`, recheck pins and
   cancellation, inspect startup evidence, and start/resume the reserved ID only
   when the classifier permits it. Add store transitions for confirmed startup and
   settlement; no arbitrary state setter. Child-intent mutation remains slice 3.
3. **Shared tracking and persisted events:** reshape the existing daemon forwarding
   helper to expose authoritative completion `Result` to a bootstrap settlement
   observer. Production runtime context requires both the actual Engine tap and
   storage; it must not silently select a no-events fallback. Synthetic facade
   tests may use an explicit legacy adapter. Before start/resume register the
   per-run broadcast and tap subscription. Filter by run ID and deduplicate by
   persisted sequence; fresh runs start at zero, resumed live delivery starts at
   the pre-resume durable watermark. Tap lag/gaps require durable catch-up or an
   explicit error, never silent loss. Completion triggers a final durable snapshot
   and flush through its captured sequence boundary before terminal publication,
   `RunFinished`, deregistration, and slot release. Do not forward handle `Terminal`
   before that flush and authoritative join. Join/storage failures are unconfirmed
   outcomes for the bootstrap observer, not fabricated successful cancellation.
4. **Cancel/recovery:** cancellation commits without waiting on a long side-effect
   guard, then signals the owner. Recheck after Git preparation and after obtaining
   the Engine handle; if start won, stop and join it. Unconfirmed settlement retains
   cancellation in attention. Startup examines all nonterminal operations under
   the same guard: absent allows first start; started resumes the recorded path;
   parked observes wake time; terminal drives durable settlement; incomplete blocks.
   Exclude every operation-owned reserved ID from legacy recovery/wake claims before
   those loops run, including unknown payload versions. Run recovery before intake,
   inbox and IPC launch sources. Shutdown stops admission and joins owned tasks;
   a grace timeout leaves recoverable durable state rather than claiming cancellation.
5. **Wiring/files:** daemon `main.rs`, `bootstrap_runtime.rs`, supervisor module,
   existing `server.rs`/`broadcast.rs` tracking, `recovery.rs` and `wake_scheduler.rs`;
   bounded core capture/state additions and persistence bootstrap-store transitions.
   The inspector/classifier module remains separately owned. No second engine,
   generic job subsystem, dependency, or raw EngineRunConfig persistence.

Acceptance includes real-Engine HumanGate IPC delivery using the production client
start ordering (including subscribe-before-start race): exact persisted request
sequence reaches the client before approval, the answer resolves that gate, and
persisted resolution/terminal events arrive before stream closure. Inject tap lag
and delayed final delivery to prove catch-up/flush. Also cover partial startup,
restart with the same IDs/OID, cancellation before/after start, competing claims,
max_active=1, parked recovery, missing runtime pins, panic or mismatched terminal,
and preserved execution edits/source-checkout isolation. Snapshot-plus-live client
reconnection and durable pending-approval reconstruction remain slice 4.


Slice 1 local evidence: schema reopen and new IPC verbs were observed RED before
implementation. Persisted-state and cancellation-attention regressions were also
observed RED and repaired. The 21 new focused tests pass. Full core, persistence,
orchestrator and daemon all-feature tests/doctests pass; strict all-target,
all-feature clippy and workspace formatting pass. Platform execution/release CI
and supervisor/continuation behavior are not implied by these local results.

### Startup inspection prerequisite review

Independent spec review followed by quality review accepted the read-only
inspector and startup classifier after repair 1/3. Inspection reads an existing
SQLite database with OPEN_READ_ONLY in a read transaction, including WAL events,
without creating/migrating storage or repairing stale registry status. Database
presence is checked independently of registry presence; malformed/inaccessible
storage is an error, never absence. Registry and event snapshots are not atomic
together; supervisor ownership must serialize its starts/resumes.

The classifier verifies the reserved run, worktree, exact input, configuration,
initial graph identity and contiguous event sequence before lifecycle folding.
Typed terminal evidence survives a later wake event and registry lag. A parked
run requires matching event and registry wake times. Repair 1's NULL/mismatched
registry timestamp test failed before the fix and passed afterward. Six inspector
tests and eight classifier tests pass; strict persistence/daemon all-target
clippy and owned-file formatting pass. These modules do not start or repair runs;
supervisor integration and restart acceptance remain pending.

The next integration test used a real Engine HumanGate and production IPC client:
it failed because the durable HumanInputRequested never reached the client.
Shared tracking must repair that observed behavior. Explicit per-run stream
failure is permitted for unconfirmed join/read failures; it must resolve the
client waiter as failure without inventing a terminal run outcome or disconnecting
unrelated runs.

Tracking review constraints: final confirmation must reject conflicting definitive
terminal records rather than accepting whichever happens to be last. A later park
or wake cannot override definitive completion/failure/cancellation. Engine's SQL
event forwarder currently exits on a read error while its tap sender remains live,
so periodic durable reconciliation is justified. Reconciliation must read only
events after its sequence watermark; repeatedly decoding the full history each
second makes long runs progressively more expensive. Final/recovery inspection
may still scan the complete snapshot to validate lifecycle consistency. Subscriber
lag at the separate broadcast-to-IPC hop must surface an explicit stream failure
or be repaired; logging and continuing silently loses approval events.

Tracker spec review repair 1/3: four tracker tests and two real-Engine IPC tests
passed, including complete request/resolution sequence before Terminal and one
failed stream leaving another run operational on the same connection. Deterministic
production-drive tap lag/gap catch-up and resume-watermark coverage remain required
before spec acceptance. Use actual SQLite with a controlled tap/completion boundary;
assert exact delivered sequence, no replay of old requests on resume, and final
publication only after durable catch-up. Full affected gates remain pending.

Runtime slice boundary confirmed during implementation: v1 requires a configured
repository-root `surge.toml`. Engine restart currently persists budget and MCP
configuration, but does not rehydrate nondefault per-run tool-loop/output-spill
policies. Bootstrap capture must explicitly reject nondefault
`tool_call_loop_guard` and `output_spill` project policies until their restart
persistence is implemented; it must never accept them and silently use defaults
on resume. Bootstrap v1 also exposes no custom human-input/stage timeout override
and rejects unsupported MCP/environment overrides. Full policy persistence,
unconfigured/new-project initialization, and dirty-checkout overlays remain open
product requirements. These are preflight restrictions, not completed features.

### Shared tracker review repair 2 evidence

The actual Engine park/resume IPC fixtures failed before the repair: the original
connection missed the fresh HumanGate request, and a fresh connection missed early
completion while its follow-up Subscribe was withheld. Resume now replaces the
connection's stream generation before starting execution; failed resume removes
that forwarder and releases admission. StartRun uses the same attachment helper.
Subscribe remains idempotent for a fast run whose preattached forwarder has already
finished. An identical second definitive terminal also failed its regression test
before confirmation was changed to reject every duplicate definitive terminal.

Tap lag/gap fixtures use a 60-second reconciliation interval with 3-second bounded
receives, so periodic reconciliation cannot satisfy their oracle. A separate fixture
keeps the tap open and silent and proves polling delivers the persisted request.
All 90 daemon unit tests and three real Engine resume tests pass. The broader daemon
all-feature suite passes with only the explicitly pending supervisor ownership
recovery fixture filtered out; formatting and diff checks pass. Strict affected
checks and independent quality re-review remain pending at this checkpoint.

### Supervisor implementation checkpoint (not final acceptance)

Tracker repair 2 subsequently passed combined strict affected-crate checks and
independent spec and quality review. The previously filtered ownership recovery
fixture now passes: ordinary recovery and wake scheduling exclude both reserved
bootstrap IDs, including unknown payload versions.

Production supervisor/runtime wiring now compiles across daemon targets. The real
Engine planning fixture completes three approval gates and launches the committed
child in its own Git worktree with one shared admission slot. Inactive Started and
Parked cancellation writes one durable abort across retry, updates registry status,
and reopens as a typed Cancelled operation. Restart after child-intent commit,
newer parent artifact isolation, live gate cancellation, and broader failure cases
are still being checked; this checkpoint does not mark the supervisor accepted.

The actual bundled roadmap contract exposed an existing downstream naming drift:
producing both roadmap.toml and roadmap.md yields logical names roadmap_toml and
roadmap_md. Bootstrap extraction and binding now recognize those exact names and
prior hyphen aliases, choose the primary TOML by explicit canonical identity, and
reject competing aliases. The required artifacts/profile contract is preserved.
Two focused regressions failed before the repair; the full planning fixture also
recorded a real binding failure before underscore support was added.

Supervisor focused acceptance checkpoint: 17 daemon bootstrap tests passed,
including production IPC admission/status/cancel, direct reserved-run fencing,
retry before recapturing changed inputs, partial-start attention, committed-child
restart with a newer parent artifact, live gate cancellation, inactive abort
idempotency/reopen, and total token-budget accounting. Startup classification now
rejects any second definitive terminal, including identical duplicates, after its
regression failed against the earlier permissive fold. Broad tests, strict lint,
and independent supervisor review remain pending.

Replaced legacy removed: `surge-daemon/src/bootstrap.rs` and its preparation-only
callback abstraction. Production supervisor submission now owns that behavior;
its real repository retry test preserves the lost-reply guarantee. Store tests
retain concurrent insert/CAS evidence. The explicit synthetic/runs-only server
adapter still refuses bootstrap operations and is named accordingly. Production
main always supplies the supervisor; NotReady now denotes unavailable validated
runtime configuration, not an unimplemented execution path. Ordinary StartRun RAM
queue/recovery/wake behavior remains because bootstrap ownership does not replace
those contracts; both reserved identities are excluded from those launchers.

Supervisor spec repair 1: cancellation attention was a dead end because normal
retry correctly rejects cancelled work. A real held-writer conflict reproduced
that failure after close/reopen. A dedicated cancellation-only CAS now returns
the original phase to Cancelling while preserving pins, run identities, and the
cancel flag; it never restores Pending execution eligibility. Retry dispatches
this route before runtime/credential preflight, allowing settlement even when no
configured execution runtime is available. The fixture confirms one durable abort,
no agent/child start, and no admission slot. All 19 focused bootstrap tests pass
following this repair. Ordinary attention retry additionally verifies Git
ownership before acknowledging restored inputs. Broad gates and independent
supervisor acceptance remain pending.

Bounded admission refinement approved during review: production bootstrap reuses
the configured `max_active` and `max_queue` values. New acceptance is serialized
with ordinary run admission by the controller's existing mutex, with no capture
I/O under that lock or the SQLite transaction. A provisional active reservation
is rolled back unless bounded insert commits; same-ID intent comparison precedes
quota rejection inside the transaction. All nonterminal journal records count,
including parked, attention, cancelling, and future payloads. The invariant is
accepted nonterminal bootstrap operations <= max_active + max_queue. With zero
queue capacity, new work requires an immediately claimable physical slot. Already
accepted parent/child continuation may wait later without becoming a new operation;
parked or attention work retains durable occupancy without holding an execution
slot. Distinct-ID barrier tests reproduced eight acceptances where two were
allowed, and zero-queue acceptance incorrectly succeeded behind an ordinary run.
The bounded repair passed distinct-ID concurrent saturation, zero-queue admission,
restart/future-version occupancy, deferred-COMMIT rollback, and inactive/terminal
release tests. Pre-FIFO-review full affected suite passed 2075 tests (26 existing
ignored; totals include watchdog child summaries); combined strict lint and
workspace formatting passed.

Supervisor quality repair 2: an incoming request could reserve a newly freed slot
before an earlier durable waiter was reconciled. The real Engine regression
observed the newer operation's SessionOpened first after the previous owner
released, even with repeated same-ID retries. Admission now queries supported
pending queued/preparing records in durable queue_sequence order and reserves the
first unreserved older run before the incoming one. Successful bounded insertion
retains that older reservation for idempotent reconciliation; insertion failure
releases it through the existing RAII guard. Active/parked, cancellation, attention,
future-version, and terminal records are not executable admission candidates.
Eligibility and full validated rows come from one SQLite query/snapshot, avoiding
an ID-select/record-reread race with cancellation. The FIFO fixture and all 17
supervisor tests pass. Final focused daemon/persistence bootstrap checks passed
44 tests; combined all-target/all-feature strict Clippy passed for ACP, MCP,
orchestrator, CLI, daemon, and persistence. The final full core/persistence/daemon/
orchestrator all-feature suite passed 2077 tests with 26 existing ignored (totals
include watchdog child summaries), and workspace format/diff checks passed.
Independent quality re-review remains the final acceptance gate.
