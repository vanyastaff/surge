# Project lifecycle goal

Status: active; no end-to-end completion claim. Baseline: `6265b59` (PR #149).

## User outcome

Surge should guide both a person starting from an idea and a person continuing an
existing repository through the whole project. Developers and non-developers can
understand the flow, planned tasks, current agent work, completed stages, results,
and problems. Real provider testing uses `codex-acp` with `gpt-6.1-sol`.

`docs/product-strategy.md` supplies the existing product decisions. Runtime owners,
accepted outcomes, route consumption, settlement and verification retain their
existing independent contracts.

## Completion ledger

| Requirement | Evidence required | Current evidence / status |
|---|---|---|
| New idea reaches a usable project | Actual desktop journey through planning, approval, execution and running the result; inspect generated project and tests | Unverified |
| Existing repository can continue from its current stage | Actual desktop journey on an isolated existing repository; conventions retained, worktree changes and usable improvement inspected | Unverified |
| Whole path is understandable | Native screenshots and interaction assertions showing authored stages, planned/conditional next steps, current work and past stages together | Home implemented; 286 scoped tests pass, final review/native QA pending |
| Current agents and their work are visible | Actual live session bound to the exact task/run; expandable feed and accurate offline state | Per-run surfaces exist; project-wide integration unverified |
| Results and completed stages remain inspectable | Multiple operations/revisions and cold recovery preserve historical tasks, artifacts, evidence and exact destinations | PR #149 supplies TaskExecutionView; full project history unverified |
| Problems and required decisions are actionable | Failure, unavailable history, disconnection and decision scenarios open the exact owner and preserve uncertainty | Home exact-owner navigation implemented; independent final reviews pending |
| Both audiences understand the interface | Plain-language native labels, technical details available, actual navigation exercised | Partial existing UI; full journey unverified |
| Real Sol 6.1 provider test | Same ACP session advertises exact ID, acknowledges mandatory selection, executes bounded daemon/MCP scenario and settles; metadata binds durable session | Actual exact Sol selection and MCP exchange observed; terminal completion not proven |

## Next implementation slices

1. **Project Home.** Pure typed projection, native landing screen and shared task
   refresh. Reuse daemon-owned task cache for both Requirements and Flow origins.
   Include pagination, generations, ownership, availability and history coverage.
   Supplement legacy history through read-only inspection, with exact repository
   identity. Keep accepted planning distinct from draft artifacts and project files.
   Reuse TaskExecutionView; actions navigate existing owners.
2. **Portable exact-model live harness.** Explicit executable/JSON arguments/model,
   graph runtime model selection, test-only protocol metadata capture, same-session
   offered/acknowledged selection before prompt, bounded child settlement. Keep paid
   provider invocation opt-in and ordinary CI controlled.
3. **Prove the complete journeys.** Run both starting points through the actual
   desktop, fix failures found, inspect usable results and recovery. Home and a
   transport smoke test alone do not retire the full outcome requirements.

### Actual journey fixtures

Use disposable, isolated runtime homes and repositories; never launch test agents
against the maintainer's working copy. Keep generated demonstration recordings and
provider captures outside the repository.

- **New idea:** a small local habit tracker, with at least two authored stages:
  creating/checking habits, then persistence and filtering. Exercise planning,
  approval, execution and inspect the runnable interface. Check behavior using a
  fixed independent acceptance list, not the agent's own summary. Add a follow-up
  idea after the first result and confirm earlier stages remain inspectable.
- **Existing folder:** a small existing application with explicit conventions,
  baseline tests and a reproducible missing behavior. Open and scan it, implement
  the requested improvement through Surge, inspect the worktree diff and run its
  acceptance tests. Check that the original checkout and conventions survive.
- **Recovery and problems:** use controlled fixtures for a required decision,
  failed attempt, offline state and cold restart. Check exact navigation and
  retained stage/task history; a live paid provider need not be deliberately
  crashed to reproduce every deterministic failure mode.

Capture actual provider-reported Sol selection, task/run identities, accepted
requirements, routing, settlement and result evidence for the real journeys.
Successful transport negotiation alone does not satisfy these scenarios.

### Discovered desktop startup gap

At baseline `6265b59`, `surge-daemon/src/main.rs` enabled bootstrap only when startup cwd or
an ancestor contains a valid `surge.toml`. Desktop `start_daemon` inherits cwd,
while `BootstrapRuntime::for_project` can resolve the project's agent catalog only
after a bootstrap runtime already exists. Consequently, opening a configured
project after an ordinary application launch does not establish planning
availability. Local commit `39ecc76` repairs this with configured-project
startup and cold-recovery regressions; native end-to-end journeys remain pending.
A configured test-launch cwd may help
diagnose transport separately, but does not retire this product requirement.

The desktop exposes default-agent selection. The generated implementation Flow
inspector also exposes provider and model choices; the initial planning graph has
no equivalent native model selection. Bundled planning profiles name Claude explicitly; changing default
agent does not retarget them. Disk profile overrides can select the Codex runtime,
but `recommended_model` remains advisory. The installed modern adapter documents
`CODEX_CONFIG` and derives its reported initial model from the provider's
thread-start response. Real planning journeys can use this supported adapter
configuration plus same-session observation, without changing production profile
recommendation semantics. Keep the strict harness's mandatory node-override proof
separate from initial provider selection, with explicit capture provenance.

A preconfigured profile fixture proves a configured operator journey, not
first-run usability. Independently exercise ordinary installed-agent onboarding:
choosing a ready provider must lead to usable planning assignments or an explicit,
actionable explanation. Do not treat manual profile edits outside the desktop as
evidence that a non-developer can complete the ordinary start path.

## Current checks

Review repair counts (shared across spec and quality stages, maximum three per
slice): startup **2/3** (restart coverage; reject symlinks to special files),
provider harness **3/3** (stale replies/flushed forwarding; typed failure diagnostics; parent proof validation),
Project Home **1/3** (all owned legacy runs remain visible; on-demand modern details; exact recorded stage/session destination). Startup spec and quality reviews are complete after both
repairs. Provider failure-observability code/reviews are complete; actual terminal success remains unproven. Home final reviews remain open.
Provider repair 2 passed independent spec and security review; owner review found
that the new parent success gate did not require the observed session, adapter
metadata or strict selection mode. Repair 3 addressed that acceptance gap and required the complete capture proof.
Repair 3 reproduced three false successes, then passed 36/36 controlled tests,
scoped strict Clippy and independent affected spec, owner and security reviews.
Diagnostics closure is committed as `fa055b3`. No fourth automatic repair is
authorized by the skill budget.

Local completed stages: harness infrastructure `514716c`; daemon startup
`39ecc76` (12 binary and 6 actual Unix daemon tests; scoped default/all-feature
strict Clippy). New native Windows daemon-child scenarios remain unverified.
These local stages do not retire full workspace or product-journey requirements.

- Behavioral RED: both actual desktop project-entry paths reach Tasks rather than
  Home. Isolated child fixtures finish setup and fail at the screen assertion.
- Behavioral RED: registry summaries with tied timestamps lack deterministic ID
  ordering, which is required for safe keyset continuation.
- New harness RED: no acknowledgement / wrong acknowledgement cannot authorize
  prompt; exact selected model must be recorded. Additional RED reproduces stale
  authorization after a later model-changing attempt.
- Native baseline screenshots were captured from a fresh build and inspected.
  Source implementation, GREEN checks, independent final reviews and real
  provider/native journeys are still in progress.
- First actual live Sol smoke failed before proving the complete scenario.
  The outer watchdog hid safe failure context along with raw provider output;
  typed private diagnostics are being added before another live invocation.
  Model availability/selection and successful real-provider completion are not
  inferred from this failure.

## Verification contract

Write externally observable failing tests before behavior changes. Home coverage
includes project-open navigation, older history alongside modern Flow work,
colliding IDs, exact decision destinations, stale project-switch responses,
pagination, offline/corrupt history, filesystem read-only opening and TaskSplit
cold recovery. Provider coverage includes absent model, wrong acknowledgement,
cross-session acknowledgement, recommendation-only selection, secret exclusion
and cancellation with surviving descendants.

Use independent pre-code plan review, spec review and code quality review. Run
repository formatting, strict Clippy, nextest and doctests with actual feature
sets. Record commands and their outcomes; do not infer full product completion
from targeted passes. Local build profiles may omit debug symbols to fit disk;
this changes neither feature selection nor test scope.

## Baseline evidence

Fresh native app built from `6265b59`; screenshots kept outside Git under
`/tmp/surge-project-journey-audit/`. Project opens Tasks with explicit offline
warnings. Plan and Results are separate destinations. Source inspection finds
newest-only legacy roadmap selection, so older operation history requires an
explicit shared projection. Missing runtime ownership is not proof of project
membership.

No new executor, scheduler change, verification sandbox or receipt authority is
part of these slices. Local commits are authorized; publishing a new PR or merging
new work requires the user's authorization for that work.

## Next slice: ordinary planning provider selection (pre-code draft)

Owner review proposes retaining an immutable bootstrap base graph and rebuilding
each project's effective planning graph from it. Adapt only untouched bundled
description/roadmap/flow authoring profiles to the persisted project default.
Preserve disk latest/versioned profiles, inheritance and all explicit node runtime,
model and effort choices. This connects the existing desktop Make default action
to planning; generated implementation Flow selectors already exist.

Use the engine graph validator and `effective_runtime_selector` through the
existing `profile_loader` facade (private implementation), shared by daemon
credential checks, handoff and execution. It returns the raw node/profile selector;
canonical registry recipe resolution remains separate. Eligibility requires exact
versioned allowlisted authoring profiles, Bundled provenance, exact singleton
chain, no extends, and no `custom_fields.runtime` entry of any kind. Immutable
base/effective graphs preserve repeated project scoping. Owner, harsh and domain
pre-code reviews are ACCEPTABLE after reshaping the public seam and eligibility. Freeze a versioned
selection-policy reference including configured default and canonical recipe ID
alongside the effective graph and per-node provenance/adaptation decisions. Restart reconstructs and compares original pins;
changes fail closed with retained IDs/capture and no provider dispatch. Existing
captures without the new policy reference require explicit configuration attention.

Principal outward RED: launch an actual daemon from unrelated empty cwd with
unmodified bundled profiles and a configured controlled default provider; prove
that the selected provider is dispatched and Claude is not. Additional criteria:
two projects and A→B→A isolation; operator/explicit override preservation; queued
cold recovery after default-only or profile-provenance change; selected credential
validation; native Make default persists config while unsaved Settings stays draft.
Focused bootstrap/restart/handoff and native tests precede full workspace gates.
This is an owner-reviewed draft, not authorization to bypass independent pre-code
plan review or evidence that actual Sol journeys have succeeded.

## Actual provider retry 2 evidence

Executed the final built smoke test binary directly (no new Cargo build), using
installed codex-acp 2.2.2 and mandatory `gpt-6.1-sol`.
`/tmp/surge-live-sol61-retry2.log`: exit 101 after 20.99 seconds.
The private bounded projection confirms offered and selected exact model, explicit
set acknowledgement, adapter/session metadata and one admitted prompt. Durable
counts: SessionOpened 1, HumanInputRequested/Resolved 1 each, StageToolReceipt 2,
OutcomeReported 1, RunCompleted 0, RunFailed 0. Outer and child cleanup confirmed.

Completion remains unproven. `binding_verified=false` means the final check was
not reached after the primary failure, not that a mismatch was detected. The
current diagnostic collapses unhandled non-success completion variants into
HostFailed and omits RunAborted/BudgetExceeded. Source inspection identifies a
compatible intentional budget-abort path (8192-token smoke budget, default Abort,
checked after routing), but does not establish that it occurred. Tracking failure
or another non-success variant remains possible. No further paid retry occurred.
The existing engine_budget_test independently covers high-usage abort behavior;
its result cannot identify the lost live terminal variant. Provider repair budget
is exhausted; do not silently launch a fourth harness repair or lower its gates.
Continue independent Home and ordinary planning-provider work while preserving
this verification gap. Real native journeys need their own complete event and
result evidence.

## Home review 1 findings

Fresh 275/275 targeted tests (254 UI + 21 persistence) and scoped strict Clippy
passed before independent 5a review. That review found two uncovered completeness
defects: only the first two owned legacy runs per batch received a visible/loadable
entry, and the shared task coordinator repeatedly loaded only the first eight
modern task details. Both require behavioral REDs and bounded continuation or
on-demand loading. The reviewed stage/session destination currently opens only
the exact run; it must preserve exact recorded occurrence identity or explicitly
report unavailability. Repair 1 is dispatched; source edits wait for the frozen
native build to finish. Full product journeys remain open.

## Native Home observation before repair 1

Built UI/CLI/daemon successfully and copied exact binaries to an isolated bundle;
manifest and hashes: `/tmp/surge-project-journey-audit/home-native-fixture.json`.
Canonical isolated SURGE_HOME was independently observed in the running process.
New app through the native folder picker initialized the empty project and opened
Home. Initial Start failed because the test bundle held UI only; adding fresh CLI
and daemon sibling binaries allowed the existing native Start action to reach
Engine Live. This does not prove the UI-only packaging helper distributes a usable
self-contained app; packaging completeness remains open.

Native Home Add an idea created a real accepted Requirements task named Local
habit tracker with four acceptance criteria. Home preserved and expanded its exact
request and criteria. Screenshots 06 (empty Home), 07 (manual workflow barrier),
08 (accepted task) were saved and inspected. The next ordinary task step required
manually pasted workflow TOML, so this is not a completed idea-to-app journey.

Reviewed correction: Home primary Plan application opens existing SpecWizard with
zero dispatch; explicit Create uses the existing bootstrap operation owner. A
separate Create task retains modern creation. Stale source-wizard/project Create
must be rejected before any submission (the previous handler lacked this guard).
Owner/domain/harsh pre-code reviews accepted this bounded correction with that
guard. It is included in Home repair 1 with behavioral REDs. Existing accepted
tasks remain untouched; a safe same-item planning bridge is a separate open design
gap, not implemented by changing this primary navigation.

Native copy observations: omit known-absent planning-file sections, preserve
unavailable sources, and move technical provenance/count definitions out of the
primary non-developer path into an explicit details disclosure.

UI-only cold reopen was exercised: quit the native process, confirm it absent,
launch again, choose the retained recent project and expand the same exact task
ID. Accepted revision 1 and all four criteria remained visible; screenshot 09 was
saved and inspected. This is not a daemon-crash recovery claim. The isolated UI
and daemon were then stopped; process inspection found no copied-bundle processes
remaining. Runtime and project fixtures are retained for the next native build.

Planning-provider slice pre-code review is complete after shared-selector and
provenance reshaping. Its outward RED launched the original Claude controlled
recipe despite the configured project default (`/tmp/surge-bootstrap-selection-red.log`).
Planning selection is committed in `71a2ffa`: final bootstrap 38/38, actual Unix daemon 8/8, shared engine/helper/handoff 33/33 and native default-click 1/1 tests passed. Default/all-feature scoped strict Clippy and independent spec, owner, API and security reviews passed. No real provider planning journey is claimed. Home repair 1 is source-frozen with 286/286 tests and scoped strict Clippy green; final reviews and publication remain pending.

## Packaged desktop startup slice

Separate bounded slice, repair budget 1/3 (bind fresh native build artifacts). Owner plan plus independent harsh and
security pre-code reviews accepted: `just bundle-ui` must build UI, CLI and daemon
in the selected debug/release profile; pass argv through positional parameters;
preflight all three executable inputs before replacing a previous bundle; package
them as siblings for the existing startup path. Preserve signing and runtime
authority semantics. Correctly escape SURGE_HOME as plist XML and test its parsed
round-trip. Tests exercise actual just-to-script argv/profile propagation, each
missing helper, spaces and XML characters, alongside existing packaging tests.
Mock Darwin/signing proves only the packaging contract. Final actual macOS Start
without CLI/daemon on PATH, with exact sibling paths/hashes, remains required.

Packaging builder reports genuine behavioral REDs (UI-only package, overwritten
old output on missing helpers, malformed plist and unsafe recipe argument/profile
handling) before edits. Targeted six tests are green; full Python packaging suite
and independent final reviews remain pending. This is not a fourth repair to the
provider harness; no provider harness code or verification gates are changed.

A separate disposable existing-project fixture is being prepared for the real
continuation journey: usable plain browser notes app, passing baseline tests,
independent search acceptance oracle outside the agent checkout that is RED before
the requested improvement. Fixture preparation is not Surge execution evidence.

Existing-project fixture prepared at
`/private/tmp/surge-existing-project-ncyfw3p5/project`, base commit
`260b7bf9251616192a0bc52b32d780b5404fde8d`. Root independently reran baseline
3/3 model tests and the external acceptance oracle: five search cases fail, two
blank-query cases pass. Oracle is outside checkout; SHA256 is
`29e81b7f0b271e2ae12b367c77b4165e6d4767e7af93cf3b20b9e3b1a1e9a758`.
The in-app browser rejected `file://` access by URL policy. No alternate browser,
server, protocol rewrite or other workaround was attempted. Browser visual/use
verification of this fixture remains unproven; this does not prevent the separate
native Surge continuation and independent result/model checks.

Packaging 5a found an uncovered freshness defect after 47/47 exact CI-pattern
tests passed: Cargo ambient/config target-directory or target settings can place
new binaries elsewhere while a hardcoded copy step packages old root/target
inputs. Repair 1 plan uses exactly one validated Darwin host from rustc metadata,
explicit Cargo native target and explicit root target-dir, then preflights/copies
only target/HOST/profile. Prebuilt mode remains unchanged. Root accepts the extra
native-target build cache cost and will coordinate disk/build checks. No new
Python runtime dependency or artifact-parser framework is introduced. Independent
affected pre-code reviews are required before edits; fresh override-aware REDs
will verify actual just recipe behavior and preserve old output on refusal.

Packaging implementation committed as `dcf0a0f` after repair 1: nine focused and
50 exact CI-pattern Python tests passed, with independent affected spec, owner,
quality and security reviews complete. Host/build/preflight failures preserve the
old bundle; copying/publication retains its pre-existing non-atomic behavior.
Actual native bundle/Start and full workspace/Windows gates remain open.

Planning-provider repair budget is now 1/3. Spec, owner and API reviews passed,
but security review found same-runtime provenance freshness missing: installing
a byte-identical disk override after capture could retain cached Bundled authority
without a daemon restart. The accepted repair compares a freshly resolved private
planning selection pin and effective graph against the cached result using the
same adapter, preserving the broader content guard. Required REDs include graph-
unchanged explicit runtime/model/effort and immutable captures/IDs/no worktrees.
Owner, harsh and security pre-code reviews accepted this observational check; it
does not claim atomic filesystem snapshots. Repair is complete in `71a2ffa`; actual native planning remains unverified.

## Publication checkpoint

The user explicitly authorized pushing the current branch, creating a PR and merging after green CI including Windows. The current branch is `codex/project-journey-home`; TaskExecutionView itself was already merged in PR #149. This PR publishes the bounded startup, provider-selection, harness diagnostics, packaging and Home implementation, while retaining the open full-journey requirements above. No scheduler or verification-receipts changes are included.

Home repair 1 final scoped suite: 286/286 (265 UI, 21 persistence), `/tmp/project-home-repair1-final-green.log`; strict all-targets UI/persistence Clippy: `/tmp/project-home-repair1-final-clippy.log`. Full workspace gates, independent Home reviews and fresh platform CI remain required before merge.

Home repair 1 passed independent 5a. Independent 5b found three bounded/evidence defects; repair budget is now **2/3**: partial or retained decisions presented as current; unbounded shared list/history RPCs retaining AppState under a silent connected daemon; unbounded accepted-roadmap reads allowing oversized or special files to block shutdown. Repair 2 must show behavioral REDs and rerun affected spec, owner, quality and security reviews. Full workspace all-feature all-target strict Clippy passed before repair 2 (2m24s); recheck affected source afterward.
