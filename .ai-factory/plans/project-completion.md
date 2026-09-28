# Surge completion

Started 2026-09-27 from `6905d00`, branch `codex/project-completion`.

User goal: finish Surge, with Agentlas-OS and Factory as references for a usable product.
User clarification: Surge must become an understandable, convenient and effective
Vibe Coding Harness for developers creating applications, and improve on the
Agentlas-OS reference. v0.1 reliability is an intermediate milestone, not the
completion definition. The target journey is idea → editable scope → execution →
working preview and verified changes → review/PR, with recoverable interruptions.
No existing requirement is retired by this plan.

## Current completion audit — 2026-09-28

This snapshot supersedes the historical implementation notes below. It records
local evidence, not a published release or a completed prompt-to-application
product. No requirement is retired. Telegram callback/token work is already owned
by its active slice; this audit does not create a parallel implementation.

### Completed and locally verified

- [x] **Native idea→app journey passes end to end (2026-09-28, Claude).** In the
  desktop app on a freshly created project: Fleet request → description →
  roadmap → flow gate (swimlane plan diagram + "Why this plan") → supervised
  implementation run (3 milestones, per-task and per-milestone verification,
  final verify: all `passed`, `run_completed`) → Preview (timer runs and pauses)
  → Changes (app files separated from Surge working files) → "Keep app
  changes" committed exactly `index.html`, `timer.js`, `test.js` and
  fast-forwarded the clean project; `node test.js` 11/11 in the project.
  Planning run `run-01M3MY1F5JQHTTT82EWT7YGHWR`, implementation run
  `run-01M3MY1F5K35F57P03QGEWE46G`. Blockers found and fixed on the way:
  Fleet dispatch bypassed the supervised path; supervisor required each
  project's agents to equal the daemon's; bootstrap gates timed out after 5
  min; replay picked the generated flow instead of the run's own graph;
  missing outcome report failed runs (now bounded reminder turns); Codex
  quota wording unrecognised (now parks); plan catalog now marks runtimes
  unavailable (exhausted quota / unconfigured env) so the generator routes
  around them; UI render loop starved input (cached plan preparation).
  Remaining: full workspace test/clippy rerun after the dev-profile change
  (disk exhausted twice during this session), plan-editing UI end-to-end
  (engine + continuation + inspector done and unit-tested; not yet exercised
  on a live flow gate), per-node model/effort override, native Windows/Linux.


- [x] Project switcher events now reach the desktop application. Previously
  `TopBarEvent` had no subscriber, so recent-project selection and the folder
  picker silently did nothing. The application subscribes when installing each
  top bar. A GPUI regression proves OpenOther opens the native directory picker
  and cancellation preserves the active project. Native macOS verification in
  the rebuilt Surge Preview confirmed picker opening, cancellation, and switching
  from the temporary acceptance project to the Surge repository; daemon ping is
  healthy. NewProject now initializes an explicitly selected empty folder with
  a main-branch base commit, README, .gitignore, and ignored local surge.toml.
  Nonempty folders and nested repositories are rejected. Three initializer tests
  and all 62 UI binary tests pass; native creation was verified in
  /tmp/surge-new-app-codex-20260928. Fresh onboarding without an existing configured
  project remains open.

- [ ] Live Codex application generation: operator restricted active providers to
  Codex and Ollama. Root default is codex-acp; 18 local profiles in ~/.surge/profiles
  override Claude with Codex. Real UI submission reached Codex, but inherited
  gpt-6-luna was rejected by ChatGPT authentication. A direct gpt-6-astra request
  succeeded, but codex-acp 0.16.0 rejected Astra because its embedded Codex runtime
  is older. A real initialize/session/new/session/prompt ACP probe with gpt-5.5
  returned OK/end_turn; the Surge Codex command now selects gpt-5.5 explicitly.
  The operator upgraded the standalone CLI to 0.158.0. End-to-end bootstrap and
  implementation acceptance still require a successful rerun. Ollama currently
  lists deepseek-v4-flash:cloud, but its inference reports retirement and pulling
  the alias reports missing manifest; it is not a verified fallback yet.
  The full gpt-5.5 run 01M3M35KW827F0NX5ACVV83QE0 reached a real apply_patch
  for description.md, then returned Cancelled/ForcedClose. The Codex session
  records apply_patch aborted after 0.1s. Investigate ACP permission selection:
  BridgeClient::pick_option matches literal allow/deny IDs and invents one when
  absent, although ACP agents may offer different IDs with standardized kinds.
  Also observed: desktop retains a stale LIVE facade after daemon restart, and
  mac-notification-sys opens a Where is use_default? dialog on OS notification.

- [x] ACP canned permission responses select the agent-offered option by
  AllowOnce/RejectOnce kind, preserving opaque IDs. Missing matching options
  return Cancelled; no invented allow/deny ID or persistent grant is emitted.
  Three focused unit tests and three permission/sandbox integration tests pass;
  strict surge-acp Clippy and all three application binaries build successfully.
  Live run 01M3M3H6YQ0R5KYHRYG6PJ7T6S created description.md, passed its
  validator, persisted the artifact and reported drafted. It then failed on
  graceful-close timeout; npx descendants remained alive. Those terminal-run
  descendants have now been stopped explicitly.

- [x] Unix ACP launches use isolated process groups; forced cleanup kills the
  group before reaping the launcher. Validated stage success survives confirmed
  forced cleanup, while unconfirmed cleanup remains fatal. A real shell child
  inherited-pipe regression passes, as do 13 ACP lifecycle tests and 10 stage
  cleanup/rate-limit tests. Strict all-target Clippy for ACP/orchestrator passes;
  CLI, daemon and UI builds pass. Windows retains child-only cleanup. Rebuilt
  daemon PID 78285 hosts native-UI run 01M3M48ESGXWSYWGW4K3ZH6FX7. Description
  and roadmap both validated, persisted and completed after forced session close;
  their adapter process groups disappeared. Both artifacts were inspected and
  approved through the native Inbox. Current live stage is flow_generator
  (session 01M3M4E3WGGQ7C1GRHZRER83R9, isolated PGID 79912).
  Run subsequently failed at flow_generator: malformed TOML (node arrays,
  missing start, scalar archetype) hit generic artifact rejection before the
  documented bootstrap validation/edit loop. The terminal run must not be
  mistaken for an active agent. Fixed routing so only Flow content from a
  flow-generator profile is deferred to the bootstrap validator; path and
  readability checks remain enforced. Regression test and all eight bootstrap
  tests pass. Bundled/local generator prompt now describes actual TOML map
  serialization; prompt snapshot refreshed. Pending: strict Clippy result,
  rebuild, resume/fork the failed run, and live retry evidence. Draft flow also
  picked unavailable ollama-verifier despite Codex-only request: correct runtime
  selection before accepting the generated workflow. Implementation acceptance
  remains open.

- [ ] Live flow repair continuation: rebuilt CLI/daemon/UI and restarted daemon
  PID 81828. Fork 01M3M4S2M7QQNS5NJWZ50QZ6YZ was marked failed by startup
  recovery (worktree lost): CLI fork creates the log but no isolated worktree,
  while recovery assumes ~/.surge/worktrees/<run>. This is a real unresolved
  recovery/provisioning defect. After daemon readiness, fork
  01M3M4VK76SP09H8CF32RMCXMN from parent 01M3M48ESGXWSYWGW4K3ZH6FX7 seq41
  resumed successfully with the test project cwd. Current session
  01M3M4VSQQ6FWE3D6E7B2SAA81 is flow_generator on codex-acp. Prompt amendment
  explicitly requires implementer@2.0/verifier@2.0 and excludes unavailable
  Ollama/Claude. Do not restart this live run while observing it. Fork's success
  path now closes child writer before parent lineage and closes parent writer;
  all eight fork tests pass. That final writer change is source-only until the
  next rebuild.

- [x] Real flow validation retry is now observed in run
  01M3M4VK76SP09H8CF32RMCXMN: parse errors at seq48 and seq58 persist
  BootstrapEditRequested, traverse e_flow_validation_failed and open fresh
  Codex sessions. Current third session is 01M3M50WFMV7HZ6AAAYVQ5E7JV.
  Generator still guesses nested enum serialization from individual parser
  errors. Added canonical bundled multi-milestone TOML to parse-error feedback,
  explicitly retaining actual roadmap/runtime/verification requirements. All
  eight bootstrap tests pass, including parsing and engine-validating the
  embedded reference. This feedback enhancement is not in the running daemon
  yet; do not restart while the current run is active.

- [x] Initial Flow Generator context also receives the same executable TOML
  serialization reference after template rendering (including disk profiles),
  preserving operator restrictions. Nine bootstrap tests pass; CLI/daemon/UI
  rebuilt successfully. Current live fourth attempt is session
  01M3M53G7TF5B73DYNJYBZ9NA3 in 01M3M4VK76SP09H8CF32RMCXMN, still on daemon
  PID 81828's previous binary. Observe its terminal result before replacing the
  daemon. The fork is an engine run, not a supervised bootstrap operation, so
  successful graph generation alone will not prove automatic child dispatch;
  full native prompt-to-implementation acceptance still needs a supervised run.

- [x] macOS UI notification setup now takes the real NSBundle identifier before
  notify-rust delivery; unbundled launches skip OS notifications instead of
  invoking the dependency's use_default AppleScript lookup. Workspace-managed
  objc2-foundation dependency, UI check/build and strict all-target UI Clippy
  pass. Native accepted-run notification no longer opens Choose Application;
  actual Notification Center delivery is not separately verified.
- [ ] Fork 01M3M4VK76SP09H8CF32RMCXMN ended RunFailed at seq80 after the
  configured three edits (last parse error: uppercase Loop). Replaced misleading
  Rust variant names in bundled/local generator instructions with lowercase
  serialized node_kind values; prompt snapshot test passes. New daemon PID86097
  includes the executable reference in initial generator context and feedback.
  Fresh native supervised run 01M3M5DDQTDHC0539HFKDS7E61 is active at
  description_author, using the existing description/roadmap and explicit
  Codex-only implementer@2.0/verifier@2.0 constraints. CUA variable
  fullRunPreview targets the updated Preview. Observe this run next.

- [x] Daemon client now drops its global broadcast sender when the socket read
  loop ends, even while the UI retains its facade. Previously the retained
  sender kept the UI's receive loop alive and its connection badge stale LIVE.
  New real-socket disconnect regression and all four daemon_resume_stream tests
  pass; strict orchestrator/daemon all-target Clippy passes. Source-only until
  next UI rebuild; native restart/reconnect check remains pending.
- [ ] Supervised run 01M3M5DDQTDHC0539HFKDS7E61 description and roadmap were
  inspected and approved through Inbox. Roadmap approval comment clarifies that
  bootstrap owns flow repair; implementation must not rewrite its active graph.
  Next stage is flow_generator with the executable serialization reference.
  Removed six orphan adapter processes whose original Preview parents were
  already terminated; the live daemon's agent processes were preserved.

- [x] User-reported Inbox schema bug fixed: bootstrap decisions select the
  immutable produced description/roadmap/flow artifact from their own run stream,
  load it asynchronously with a 2 MiB limit, and render Markdown content instead
  of JSON response schema. Loading/read errors are explicit. Bootstrap gates
  expose Approve / Request changes / Reject even when free-text schema omits an
  enum. New document-selection regression passes; all 63 UI tests, strict UI
  Clippy and build pass. Preview updated and reopened on current project;
  no pending gate at inspection time, so native document rendering still awaits
  the next live gate. CUA binding inboxPreview. Daemon remains PID86097.

- [x] Fleet no longer invents sample runs when the daemon run list is empty.
  It renders application-creation guidance and the real task composer instead.
  Connection status comes from the daemon link, and a completed workflow is
  labelled completed rather than claiming a Git merge. Three Fleet regressions,
  strict UI Clippy, formatting, and the UI build pass. Native macOS verification
  confirmed a live daemon with zero runs displays zero counters and onboarding,
  without fictional cards or review requests.
- [x] Removed sample fallbacks and their runtime data from Inbox, Runs, and
  Roadmap. Missing data stays empty; disconnected Inbox does not claim that no
  decisions are pending. Added regressions for all three data projections. All
  59 UI binary tests pass. Native macOS verification shows no invented approvals,
  run rows, or milestones when opening these screens with an empty live daemon.

- [x] Detached daemon startup now owns persistent log handles and reports early
  child exit. Run execution explicitly closes its persistence writer. A fresh
  detached daemon completed the terminal fixture with no unclosed-writer warning.
  Actual restart exposed a CLI deadline of 10s versus the daemon's default 30s
  grace. Restart now allows 45s by default and accepts `--wait-timeout-secs` for
  custom grace settings. A real Unix subprocess regression with an 11s grace
  verified replacement PID and IPC health. CLI and daemon builds passed; the
  main daemon was replaced and desktop Computer Use confirmed `DAEMON · LIVE`.

- [x] Desktop live bootstrap reached Claude ACP on 2026-09-28 but failed on
  subscription quota; no application was generated. The captured quota phrase
  was missing from classification, and confirmed forced session cleanup erased
  typed rate-limit errors. Both regressions failed before repair. Classification
  now recognizes the observed phrase; confirmed reaping retains the original
  failed-stage type, while unconfirmed cleanup remains fatal. Engine tests prove
  durable parking, forced SessionClosed evidence and one provider dispatch even
  with a retry-routing hook. Affected stage/capacity/gate tests: 33 passed; ACP
  classification: 6 passed. Strict all-target/all-feature CLI/ACP/orchestrator
  Clippy passed. The provider's named-timezone clock reset is not parsed; the
  configured backoff applies. Live application delivery remains unproven.

- [x] Desktop task submission now uses `StartBootstrap` with the form's stable
  operation ID, stores a small local recovery index, resumes status polling on
  daemon reconnect, follows planning into the reserved implementation run, and
  cancels the complete operation rather than only its current run. The Runs
  cockpit identifies bootstrap phase and attention state. A configured USD cap
  is rejected with an actionable message because cross-run hard USD accounting
  is not implemented; it is never discarded. Native UI suite: 53 tests passed;
  all-target/all-feature check and strict Clippy passed; index restart/corruption
  cases passed. Manual Computer Use confirmed the form's honest planning copy,
  multiline whitespace preservation, and draft retention after navigating away.
  The controlled `real_planning_continues_to_child_with_one_admission_slot_and_isolation`
  supervisor fixture also passed against SQLite and the Engine; no live provider
  was involved.
  This does not yet prove daemon admission, operation recovery, approvals, or
  generated application delivery through the desktop.

- [x] Release archive implementation and archive-content validation independently
  accepted: four target archives contain CLI, daemon and documents; checksums,
  tag/version checks, extracted-binary smoke steps and offline packaging tests
  exist. Python 3.12 tests and actionlint passed. macOS arm64 debug archive smoke
  passed; this is not native release-matrix evidence.
- [x] CLI foreground lifecycle, durable outcome propagation, HumanGate stop,
  bootstrap EOF/resume watermark and explicit watch stream failure handling.
  Independent reviews and affected tests/strict gates passed; the later watch
  repair passed all 99 CLI unit tests.
- [x] Durable daemon bootstrap slices 1–3 and shared durable-event tracker accepted.
  Production main supplies the supervisor. Pinned repository/common-dir/OID,
  immutable child artifact intent, restart reconciliation, cancellation settlement
  and retry, bounded admission and FIFO reservation have actual SQLite/Engine/IPC
  regressions. Final supervisor repair gate: 2,077 passed, 26 existing ignored
  (includes watchdog child summaries), plus combined strict/fmt/diff checks.
  Desktop adoption is separate and remains open.
- [x] ACP worker lifecycle, authoritative prompt settlement and process cleanup;
  stable-wire stage MCP receipts/human calls; SDK 2.2.0 migration; bounded helper
  transport hardening accepted in their respective reviews. See
  `acp-worker-lifecycle.md`, `acp-stage-mcp.md`, `acp-sdk-2.2.md` and
  `mcp-stage-hardening.md`. Hardening evidence includes 41 MCP tests, 14 actual
  Engine ACP/MCP cases, strict gates, Rust 1.96 check and Windows cross-check.
  Native Windows and supported live-provider execution are not implied.
- [x] Telegram durable request identity/recovery prerequisite accepted after
  unreadable-journal isolation repair. Tests prove stale requests cannot resolve
  a later gate and a corrupt run cannot starve healthy missed-card recovery.
  Recorded gates: CLI/core/Telegram 1,098; orchestrator subset 361; daemon subset
  112; ACP/MCP compatibility 14; repair Telegram 115. Native UI check and combined
  strict/fmt gates passed. Callback execution and pairing are still open.
- [x] Editable native planning form and GPUI Kit upgrade retain real input/drafts,
  reject duplicate/blank submission, and expose submission failure. Default UI
  suite 55 passed; actual native offline draft/navigation/textarea repairs passed.
  Rust 1.96 UI all-target check passed; the later Telegram compatibility gate also
  passed UI all-target/all-feature check and combined strict lint. These prove
  offline UI behavior, not durable bootstrap submission or application delivery.
- [x] Controlled live-smoke fixture exercises real daemon + ACP + stage MCP +
  exact human nonce response and durable completion. Oracle repair has observed
  RED→GREEN against committed checkout mutation and extra terminal evidence;
  both pinned HEADs and exact event counts are checked, even after failed runs.
  Controlled run, strict lint and formatting passed. The live Codex attempt did
  not pass; do not reinterpret controlled-provider evidence as live success.

### Remaining blockers, in dependency order

1. **Telegram callback and token/pairing path — active work, do not duplicate.**
   Finish one production update stream with acknowledged callbacks, chat/request
   authorization, exact typed gate identity, edit/reply correlation and inbox
   routing. Close token consume/pair atomicity and coherent bot-token source
   handling. Prove through production routing + SQLite + local HTTP, then record
   any separately authorized live Telegram validation. `/run` is still explicitly
   deferred; Abort/Snooze callback branches are still deferred at this audit.
2. **Live provider acceptance.** One Codex adapter attempt ended after 14.87s with
   an actual failed run classified as a timeout. Fixture deadlines were not the
   branch returning this failure; no deterministic local 15s limit was found.
   Raw provider/auth material was not read or printed, and temporary storage was
   removed. More precise durable phase evidence was therefore unavailable for
   that attempt. No missing-auth conclusion or supported-Codex claim is justified.
   The new fixture emits structural counters for future authorized attempts.
   Claude's earlier usage-limit failure also does not establish successful live
   execution. No provider retry was performed during this audit.
3. **Remaining daemon/application lifecycle contracts.** Ordinary `StartRun`
   acceptance still uses RAM `PendingStarts`; it is not the durable bootstrap
   queue. Intake completion still drops lagged notifications and lacks a startup
   reconciliation sweep. New-application initialization, dirty overlays, arbitrary
   project runtime capture, full resume-policy persistence and reliable hard USD
   budgeting remain outside bootstrap v1. Keep explicit fail-closed restrictions
   until implemented; do not delete ordinary recovery/CLI console paths without
   replacement and parity evidence.
4. **Desktop end-to-end application journey.** Desktop submission now calls
   durable `start_bootstrap`, indexes operation identities locally and refreshes
   typed daemon status on reconnect. Still prove actual submission/recovery against
   a configured daemon, reconstruct pending decisions before live updates, retry
   from attention, show exact result artifacts and checks, and exercise a working
   app preview plus review/PR. New-project creation still shares the open-folder
   path; implement repository initialization and detect startup/test/service
   readiness before claiming idea-to-app support. Manual native testing was
   limited to current welcome/planning screens because the daemon was offline.
5. **Native platform/runtime gates.** Run supported native Windows named-pipe,
   process cleanup and MCP helper tests; Windows MSVC cross-check is compilation
   evidence only. Complete native Linux/macOS Intel/Windows release and relevant
   desktop checks. Refresh workspace-wide final gates after active changes; old
   broad-suite results are not a claim about the final combined source snapshot.
6. **Packaging/release delivery.** Execute the configured four-platform optimized
   build, archive, checksum and extracted CLI/daemon smoke matrix on native
   runners; verify installation in a fresh environment and accurate support/docs.
   Package-manager distribution remains pending. Release publication is a separate
   authorized external action and has not occurred.

### Production stub/deferred-path audit

Paths/lines below are from the current source snapshot; active Telegram edits may
move them. `rg` matches in tests or UI placeholder text are not product gaps.

| Location | Current meaning | Removal decision |
|---|---|---|
| `crates/surge-telegram/src/cockpit/production.rs:524` (`DeferredRunStarter`) | Telegram `/run` returns an explicit deferred error | Real advertised gap; replace when start wiring is covered, do not merely delete the requirement |
| `crates/surge-telegram/src/cockpit/callback.rs:315` | Abort/Snooze return `NotImplemented` | Active callback slice owns replacement and production tests |
| `crates/surge-telegram/src/commands/pair.rs:45` | Token consume and pairing writes are separate | Real atomicity gap; its claim that live polling is absent is stale commentary |
| `crates/surge-daemon/src/main.rs:1536` | Legacy inbox is outgoing-only; one cockpit poll must route inbox callbacks | Retain single-poller ownership; remove the deferral only after callback routing replaces it |
| `crates/surge-daemon/src/intake_completion.rs:44` | Lag can strand an intake ticket Active; startup sweep absent | Real recovery gap, not dead code |
| `crates/surge-daemon/src/server.rs:148` (`PendingStarts`) | Ordinary-run queue remains RAM-only | Cannot remove until ordinary StartRun durability/recovery is replaced and tested |
| `crates/surge-daemon/src/bootstrap_supervisor.rs:104,161` | `NotReady` when validated runtime is unavailable | Correct fail-closed runtime guard, not an unimplemented supervisor |
| `crates/surge-daemon/src/server.rs:361,376` | No-supervisor runs-only adapter and defensive unmatched dispatch arm | Adapter is explicit; production main uses `run_with_supervisor` at `main.rs:410`. Do not classify as disabled production bootstrap |
| `crates/surge-daemon/src/server.rs:81` | Doc says production callers use `run_runs_only` | Stale comment after supervisor wiring; documentation correction is safe, adapter deletion is not yet justified |
| `crates/surge-daemon/src/main.rs:449,535` | Generic placeholder inbox-card fallback | Existing fallback behavior, not proof of an unimplemented engine path |
| `crates/surge-daemon/src/wake_scheduler.rs:369` | `stub: start_run not used` | Test-only facade; retain |
| `crates/surge-cli/src/commands/profile.rs:341,360` | TODO text emitted into a user-authored profile template | Intentional scaffold, not production `todo!()` |

The superseded daemon `bootstrap.rs` preparation-only helper was already removed.
The local CLI bootstrap console and ordinary run recovery are not fully replaced;
removing them now would reduce working functionality. No production `todo!()` or
`unimplemented!()` was found in the audited daemon/Telegram/CLI command surfaces.

## Historical baseline observations (superseded where noted above)

- `.autopilot/state.js` marks competitive-waves finished; do not restart that run.
- `.ai-factory/ROADMAP.md` is stale in places: budget guards and Telegram polling
  are implemented, while some milestones marked complete contain deferred paths.
- Agentlas-OS README advertises installation, Desktop and independent verification.
  These are reference capabilities, not independently verified competitor quality.
  Source: https://github.com/agentlas-ai/Agentlas-OS (read 2026-09-27).
- `RTK.md` referenced by supplied instructions is absent from the repository and
  checked parent/user instruction locations.
- Telegram production routing discards successful callback outcomes and ignores
  reply text. Shared callback admission does not enforce card chat ownership.
  Gate cards use attempt zero and omit call IDs; closing them without fixing
  request identity would break repeated gates. Full callback work must test
  repeated requests and late replies as well as successful Edit.

## Historical verification log

- `cargo fmt --all --check`: passed on baseline.
- `cargo test --workspace --exclude surge-ui --no-fail-fast --quiet`: passed
  (exit 0; default tests and doctests, explicitly ignored tests not executed).
- CLI lifecycle repair planned and independently reviewed: local runs own their
  completion; only daemon runs may detach; failures/aborts/parked states do not
  return success. Local engine gates fail with actionable guidance until a
  schema-aware cancellable console path exists. Bootstrap EOF must not approve.
- CLI HumanGate regression exposed an engine prerequisite: gate waits ignore
  cancellation. Two independent plan reviewers accepted passing cancellation
  into the decision wait (not dropping a whole stage during event writes),
  bypassing failure hooks on cancellation, and requiring durable abort evidence.
  Owner repair implemented: Engine stop regression failed before the fix and
  passed afterward; human-gate unit subset 11/11 passed. Full CLI suite passed
  (91 unit tests plus integration binaries, including six lifecycle cases).
  Concurrent decision/stop integration passed. Strict clippy for CLI and
  orchestrator (all targets) passed before review repair 1. Spec review found
  cancellation helper discarded a task's returned failure; repair and tests for
  bootstrap driver settlement/stale resolution completed in repair 1. HumanGate has no
  configurable on_error hooks; do not invent a fixture to claim hook coverage.
  Spec repair 1 independently accepted. Quality review found historical approval
  replay on bootstrap resume; repair 2 adds a persisted-sequence boundary and
  regression for a resolved historical gate followed by a current gate.
  Repair 2 independently accepted; actual two-gate resume regression showed
  RED then GREEN. Full CLI suite now 95 unit tests plus integrations passed.
  Parent workspace suite after repair 1 passed; repair 2 is CLI-only and received
  its full affected suite. Final strict CLI clippy passed; CLI + orchestrator strict checks passed
  before the CLI-only repair. Lifecycle slice COMPLETE locally with independent
  spec and quality verdicts; full AFK journey remains open.
- Release review round 1 found that collection must validate archive contents,
  not merely nonzero file sizes. Repair independently accepted (`COMPLETE`).
  Python 3.12: five tests, including 17 malformed-archive cases, passed;
  actionlint passed both workflows. Native matrix remains unexecuted.
- Native macOS arm64 archive smoke passed using baseline debug binaries: packaged,
  extracted into a fresh temporary directory, both `surge --version` and
  `surge-daemon --version` exited zero. This does not verify release optimization
  or the other three platforms.
- Real Claude ACP doctor: spawn/handshake passed; prompt failed due to provider
  usage limit. This does not count as live execution proof.
- Real Codex ACP doctor: stopped the smoke process group after roughly 3m20s
  without a result. Shutdown diagnostic showed it was waiting in ACP
  `new_session`; no successful prompt evidence. Investigate adapter/session
  timeout handling before declaring this runtime validated.

## Interface slice 1 — historical scope (implemented; end-to-end work remains)

Maintainer scout and independent plan reviewer: ACCEPTABLE, lean review because
this is one UI crate reusing an existing daemon API. Replace the fake spec wizard
with editable multiline input and daemon-hosted bootstrap planning. Preserve
exact input, project/config capture and failure drafts; disable duplicate submit;
ignore stale async responses after project/wizard changes. Acknowledge only queued
or accepted planning, not durable admission or completed implementation. Real
GPUI input/render checks and submission-boundary failure tests required.

At this slice's start, daemon-owned isolated continuation, durable queued intents
and recoverable approvals were prerequisites. The daemon and request-identity
prerequisites now exist; UI adoption remains open. This interface slice alone does
not fulfill the end-to-end journey. Agentlas source audit and target acceptance are in
`docs/vibe-coding-harness.md` (source inspection, not an executed comparison).

## Next dependency: durable daemon-owned application workflow

The revised bounded design is in [daemon-bootstrap.md](daemon-bootstrap.md).
It supersedes the broader sketch below: bootstrap-specific durable operations
leave legacy `StartRun` unchanged. Independent re-review accepted the bounded design; slices 1–3 are now accepted.
The sketch below is historical; the current audit above governs status.

Historical sketch: the first adversarial review required the refinements below.
They are resolved in the accepted `daemon-bootstrap.md`; that document governs
implementation. Preserve crash-safe transitions/idempotency, a credential-safe
persisted payload, OID-based Git preparation, and typed durable parent outcomes.
Do not serialize every `EngineRunConfig` field blindly: MCP env values may carry
credentials and memory_store_path is a test-only override. Operation budget must
not silently reset when moving from planning to implementation.

Use the existing scheduler, Git manager, event storage and materialization helper;
do not introduce a generic job subsystem or infer orchestration from a graph name.
Add an explicit bootstrap submission operation with a durable launch record.
Persist fixed planning/implementation IDs, original project, exact execution
worktree paths, frozen base/config/graph, queue ordering and cancellation intent
before acknowledging. Keep persistence independent of orchestrator types through
a versioned daemon-owned payload at the storage boundary.

Dependency order:

1. Durable admission in registry storage: retries with the same identity return
   the existing operation; conflicting inputs reject. Rebuild the queue after
   restart and reuse one tracked-start helper for immediate and queued starts.
2. Isolated preparation and recovery: `surge-git` creates one worktree per run
   from a frozen base. Reconcile Git identity after interrupted preparation, not
   just directory existence. Preserve failed work for inspection.
3. Daemon bootstrap continuation: reuse `materialized_run_from_completed` and
   `EngineRunConfig.bootstrap_parent` to transfer artifacts. Persist child intent
   after authoritative successful parent completion; release parent admission
   before child admission so a one-slot daemon works. Parent failure/abort cannot
   start a child; a parked parent stays pending.
4. UI lineage and approval recovery: follow saved parent/child identities,
   distinguish planning versus implementation status, reconstruct outstanding
   requests before live updates, deduplicate by persisted sequence.

Known traps: `bootstrap_parent` currently seeds artifacts but does not persist
lineage. PendingStarts is memory-only. Generic recovery guesses a full-ID worktree
path although Git supports configured short-ID paths; `RunStarted.project_path`
already records the actual execution path. Separate source-project identity from
execution path. Broadcasts may trigger reconciliation but cannot be its truth.
Cancellation must be durable across queueing, preparation and parent/child races.

Required failure-injection tests use real SQLite and daemon restart boundaries:
reply lost after acceptance, queue restart order, Git preparation interruption,
parent complete before child intent, child started before supervisor update,
duplicate completion/reconciliation, cancellation racing child admission, parking,
invalid artifacts, missing worktree, incomplete startup log and failed storage.
No scenario may silently execute in the source checkout or fabricate completion.

### Agent protocol critical path

The user explicitly prioritized a working daemon and working agents before further
interface expansion and authorized updating ACP and Rust when evidence requires it.
Source audit showed the present injected-tool model is not a real provider
contract: filtered `ToolDef` values are only reported in a local event, and
`reply_to_tool` cannot return a result over ACP. Controlled notification mocks do
not prove provider tool discovery or a human-answer roundtrip. The accepted design
and red-first acceptance are in [acp-stage-mcp.md](acp-stage-mcp.md): first attach a
real stable-wire stdio MCP helper and prove result-bearing Engine behavior, then
migrate the Rust SDK from 0.10.2 to the published 2.2.0 in a separate slice. SDK
2.2.0's declared MSRV is below the current project MSRV 1.96, so raising Rust is
permitted but not required for this dependency. Stable ACP v1 remains the product
wire target; draft v2 and unstable native MCP-over-ACP cannot be the sole provider
path.

### Adversarial refinements required before the daemon slice

- Define transitions and their SQLite transaction boundaries, immutable request
  fingerprint, stable IDs and monotonic cancellation. Identical retries after
  terminal/cancelled states remain idempotent; conflicts cannot overwrite input.
- Git preparation needs a commit-OID API (the current API resolves a local branch
  at creation time), plus branch-created/worktree-not-created reconciliation.
  Define dirty-source and empty-repository behavior. Preserve unrelated Git data.
- Persist only an allowlisted versioned payload, with credential references and
  explicit handling of unresolved credentials. Reject unsupported raw credential
  payloads. Explain how profile/registry/capacity settings actually reach the
  engine: storing a project config is not sufficient to freeze daemon globals.
- Parent eligibility comes from typed durable success. The legacy recovery
  `failed: bool` cannot authorize continuation. Commit child intent and lineage
  together; reconcile the gap after parent completion.
- Commit cancellation before acknowledgment. Child admission races against that
  durable state; if admission wins, stop the reserved active child. One-slot
  scheduling, parking and queue order must remain correct after restart.
- Select the child's base explicitly; do not accidentally copy arbitrary
  planning-worktree changes. Retain required parent artifacts through seeding.
- Enforce a shared operation budget from durable usage; zero remaining cannot
  mean unlimited. Specify unknown-cost behavior under hard limits.
- Keep legacy records separate. Unknown payload versions, absent run DBs,
  initialized logs, incomplete startup and actual execution are distinct states.
  Partial startup cannot be treated as permission to start a duplicate run.

Add restart/race tests for all these refinements before calling this design
accepted. New-app initialization remains a product requirement even if the first
daemon implementation supports only existing configured Git repositories.


Native UI environment: first default compile stopped before tests because Xcode's
Metal Toolchain was missing. Installed it with
`xcodebuild -downloadComponent MetalToolchain`; `xcrun metal --version` passed
(Apple metal 32023.864). A supported GPUI runtime_shaders CLI feature was used
for the initial RED test; final default-feature verification subsequently passed.
No repository dependency feature was changed for this workaround.

### 2026-09-28 verified progress

- Desktop planning form: final default native suite passed (46 unit/render tests,
  5 integration tests); strict all-target/all-feature UI clippy and formatting
  passed. Independent spec and quality reviews accepted the final diff after one
  repair. The repair had an observed failing regression: a late accepted response
  followed by explicit OpenRun must consume only the matching accepted draft,
  allowing a fresh task. This is planning submission evidence, not end-to-end
  implementation or daemon recovery evidence.
- Factory deep research is saved in `docs/factory-product-model.md`: 15 primary
  documentation sources plus lifecycle cross-checks, distinguishing documented
  behavior from observed runtime results. No comparative performance claim.
- macOS System Settings shows Codex Computer Use enabled for Device Control and
  Data Access and Screen & System Audio Recording. Reading, clicking and capturing
  System Settings succeeded. Surge Preview still times out in `cua.getApp`, both
  with a shell launcher and a direct binary bundle. Permission absence is therefore
  not the established cause; actual Surge screen interaction remains unverified.
- Locked GPUI 0.2.2 source has no matches for `accessibility`, `accesskit`, or
  `NSAccessibility`. This observation does not apply to newer GPUI: upstream issue
  https://github.com/zed-industries/zed/issues/61925 describes existing semantic
  accessibility support and a stable-identifier improvement. Evaluate actual
  native automation before choosing an upgrade or migration.
- Isolated egui/eframe prototype passed native Computer Use focus/paste/submit,
  exact multiline whitespace and Unicode echo, select-all/delete and empty-input
  rejection. Direct typeText/setValue remain unreliable. Build, strict clippy,
  formatting and two pure tests passed. No Surge dependency changes or inspection
  server. Evidence and migration criteria: `docs/ui-automation-evaluation.md`.
- Durable operation slice 1: core validation and initial store tests are green;
  final daemon/IPC and strict affected gates remain pending. Parent spec review
  found that cancellation needing operator attention could not be represented
  without clearing the cancellation flag. Repair dispatch 1 of 3 retains that
  monotonic flag and phase/run/reason through attention and storage reopen; it
  must not permit queueing, admission or retry to revive execution.
  Repair 1 passed 13 focused persistence tests; repeated cancellation also retains
  the attention reason. Core validation (3), schema reopen (1), daemon preparation
  (2), and real disabled-IPC routes (2) also passed. The storage-failure fixture
  rejects at COMMIT after mutation, then verifies the original record remains.
  Parent independent spec and quality reviews accepted slice 1 after repair 1.
  Full affected tests/doctests passed: 1,996 passed, 26 existing ignored, 0 failed.
  Strict affected all-target/all-feature clippy, workspace formatting and diff
  checks passed. Production bootstrap execution remains disabled until the
  supervisor/continuation exist.
- Git isolation prerequisite: full crate tests passed (65 unit, 11 integration;
  2 existing ignored tests were not run). Thirteen new pinned-worktree cases cover
  frozen OID, branch-created interruption and real reopen, clean/untracked/empty
  preflight, identity-preserving reuse, and conflicts without destructive repair.
  Parent independent spec and subsequent quality reviews accepted the diff;
  strict clippy and owned-file formatting passed. Disabled or unsupported creation reflog settings
  are rejected before branch mutation because observed libgit2 behavior otherwise
  fails to record the ownership marker. Existing logging configuration is preserved.
- GPUI Kit 0.7.0 native parity prototype passed Computer Use and its three tests,
  strict clippy and formatting. A copied actual planning form with only framework
  adapters also passed native exact-input, pending duplicate prevention, rejection
  retaining draft, retry/accept and OpenRun checks under a clearly labelled
  controlled host. This does not replace production async/daemon tests. Product
  upgrade direction retains existing screens and is undergoing pre-code review.
- Build storage: root Cargo artifacts reached 56.1 GiB and disk free space fell
  below 2 GiB. After all affected tests finished, `cargo clean` reclaimed that
  reproducible cache (exit 0; 45 GiB free). Source, test evidence logs, and native
  preview bundles were preserved. Subsequent builds use process-local
  `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0`;
  repository profile settings remain unchanged. Inactive egui temporary target
  was also removed while its source and bundle were retained.
- Startup inspection/classification prerequisite passed independent spec and
  quality review after one repair: parked registry wake time must match the
  durable event. Six read-only storage tests and eight classifier tests pass,
  with strict affected all-target clippy and formatting. This is preparation for
  supervisor recovery, not completed production recovery.
- Real Engine HumanGate IPC regression failed before shared tracker changes:
  durable approval request was never delivered to the client. The test cleans up
  and joins the run even on failure. Shared tracker implementation is in progress.
- GPUI Kit product upgrade passed pre-code review and now uses the pinned 0.7.0
  dependency while preserving existing screens. First compilation is in progress;
  production native automation and Rust 1.96 verification remain pending.
- ACP readiness investigation found coupled control-flow defects, not just a
  missing CLI timeout. Worker handshake has no deadline and starts stderr draining
  only afterward. Its sequential prompt blocks queued permission replies; the
  engine agent stage also waits for prompt completion before reading permission
  events. A map borrow held across the prompt can conflict with the independently
  running child waiter removing the session. Shutdown currently aborts that waiter
  after sending kill, so its terminal event is not proof of process reap; Drop can
  synchronously join forever. A cohesive worker/agent-stage repair is being designed
  with a real controlled ACP permission interaction as its outer acceptance test.
  Preserve prompt completion/error semantics and commit outcome/verification only
  after confirmed prompt success. No runtime fix or passing acceptance is claimed
  yet; the earlier live-agent hang's specific cause remains unproven.
- Telegram production follow-up is source-audited in
  `telegram-production-approvals.md` (draft, not implementation complete). The
  single polling stream currently leaves outgoing inbox buttons unrouted, and
  cockpit callback results/Edit replies are discarded. Acceptance must exercise
  ProductionRoutes with SQLite and a local Telegram HTTP fixture, including
  repeated gate identity and restart, rather than only pure callback helpers.
- CLI watch transport repair completed locally: three behavioral regressions
  first failed because StreamError, closed-before-terminal and subscriber lag
  returned success. All four focused cases now pass, including the retained
  explicit-terminal observation behavior. All 99 CLI binary unit tests pass;
  strict all-target CLI clippy and owned-file formatting pass. Independent quality
  review accepted the localized private-helper repair. Watch always attempts
  unsubscribe before returning its original observation error; no durable run
  outcome is fabricated. This does not replace the pending real IPC tracker tests.
- Actual upgraded product preview now opens through Computer Use. Native recent
  project opening, exact multiline input, offline submission error, draft restore
  and filtered palette navigation passed. This is the product executable from the
  all-target test build, not the copied-form harness; default-feature/MSRV gates
  remain pending. Native repair 1 fixes Fleet's misleading AX action label;
  repair 2 fixes a one-row Textarea clipping multiline input. Their source fixes
  need re-verification. Details and binary hash: `docs/ui-automation-evaluation.md`.


UI slice RED observed in a real GPUI test window: typing/pasting a multiline
request then submitting the legacy wizard emitted an empty description instead
of the exact supplied text. The assertion failed at runtime (not compilation).
Implementation and final default-feature checks subsequently passed, as recorded
above. Initial framework compilation used the supported runtime_shaders feature.


Historical strict project CI gates: `cargo clippy -p surge-core --all-targets --all-features
-- -D warnings` and `cargo clippy -p surge-acp --all-targets -- -D warnings` both
passed before the new bootstrap contracts. Recheck affected gates after the
durable-operation changes. Cargo ownership is coordinated per active slice; this historical note grants no current build lock.

### Inbox document review verification — 2026-09-28

- Bootstrap Inbox cards now read the immutable artifact for the pending stage,
  render its Markdown, and expose Approve / Request changes / Reject. Removed
  response-schema and redundant outcomes evidence. Reads run off the UI thread
  with a 2 MiB limit and visible errors.
- Run subscriptions hydrate persisted events through a read-only persistence API;
  sequence deduplication prevents snapshot/live overlap from duplicating events.
- Native Preview restart restored the pending description gate and actual document.
  Screenshot verified headings and long-line wrapping inside the review pane.
  Verification fork 01M3M6E51D8XP4Q5NDAZZSX4QM was stopped after the check.
- Checks: 63 UI tests passed, then the added hydration-overlap regression passed;
  strict all-target UI/persistence Clippy and UI build passed.
- Flow Generator now receives exact Artifact and LoopItem iterable serialization
  examples, tested by deserializing the actual enum. Daemon includes these examples;
  successful end-to-end generation/implementation with this revision remains pending.
- Remaining runtime issues: CLI-resumed runs require UI restart for discovery;
  raw fork worktree recovery is incomplete. Do not count the app-creation goal done.

### Resume discovery and live workflow diagnosis — 2026-09-28

- Resume IPC and recovery resume now publish RunAccepted after successful tracking
  and before forwarding completion. An independent socket observer regression
  first failed with RunFinished as its first event, then passed. Both paths are
  covered; six daemon_resume_stream tests and strict daemon Clippy passed.
- Live Codex fork 01M3M6MKHM6Q15NAK32RWH97P1 reached PipelineMaterialized /
  flow_gate after fixing failure exit_code, overlong edge key, and missing producer
  across four attempts. It was stopped before approval: syntactic validity did
  not establish executable semantics. Generated graph reintroduced roadmap_planner
  and reads roadmap.milestones, which that profile does not produce.
- Confirmed next root fix: bootstrap_continuation seeds artifact named roadmap,
  but IterableSource::Artifact requires an in-graph producer and archetype validation
  demands a literal roadmap.milestones name. Add an explicit RunArtifact source
  (name=roadmap, jsonpath=milestones), coordinate loop resolver, snapshot, generator
  examples and topology checks, and verify admission seed existence. Handle legacy
  Markdown roadmap via existing roadmap document parsing, not guessed TOML.
- Do not fix this by inserting a fake planner or weakening node-reference validation.
  No successful app implementation is claimed.

### Approved roadmap loop source — 2026-09-28

- Added IterableSource::RunArtifact (name=roadmap, jsonpath=milestones), snapshot
  roundtrip support, archetype recognition and generator reference/profile updates.
  Existing node-produced Artifact validation remains intact.
- Engine admission now checks unique seeded loop artifacts and array selectors
  before creating run state. Loop entry resolves relative paths against the actual
  worktree and verifies recorded hashes before parsing. The real engine regression
  first exposed cwd-relative reads; after fixing those, the seeded roadmap loop
  executes the exact two expected milestones and preserves nested task data.
- 349 orchestrator unit tests and six static/seeded loop integration tests passed.
  Strict all-target core/orchestrator Clippy passed after fixing one doc warning.
- Deliberately reject Markdown as executable loop data pending lossless conversion
  and approval. Existing Markdown parser can silently omit accepted document forms;
  do not claim general Markdown support or normalize approved text lossily.
- Local flow-generator prompt updated while preserving codex-acp runtime; previous
  profile backed up at /tmp/surge-flow-generator-before-run-artifact.toml.
- New live generation and supervised child implementation still require verification.
- Deployed daemon PID 99847, ping OK. New live Codex generation fork
  01M3M7ESV21WVQHEG356P8P1ZS resumed from approved roadmap at seq 41 in
  /tmp/surge-new-app-codex-20260928. Check this existing run before starting
  another; do not restart daemon while raw fork is active (worktree recovery issue).
  Prompt also requires Codex-only final review and populated profile bindings.
  Next audit must inspect implementer/verifier spec bindings and current loop-task
  context: profiles require spec, previous generated graph had bindings = [].

### Current iteration prompt delivery — 2026-09-28

- AgentStageParams now receives borrowed execution frames from run_task. Post-render
  prompt context includes outer-to-inner loop node, variable, current index and
  current item, preserving literal template-shaped task content. Explicit profile
  bindings are unchanged; loop context is not silently substituted for a required spec.
- Real agent-stage regression proves nested milestone/task delivery and exclusion of
  a previous item. Real engine seeded-loop regression now dispatches agents and
  confirms the final prompt contains only the current iteration.
- Testing exposed another prior runtime mismatch: terminal=true on an Agent outcome
  does not end the loop body; routing still requires an edge. Bundled multi-milestone
  reference now has explicit terminals/edges for both nested bodies. Added an exact
  Binding/source TOML example tested against the real Binding type.
- Checks: 349 library tests, five agent-stage tests, six seeded/static-loop tests pass;
  strict all-target orchestrator Clippy passed. Nested-order assertion added afterward
  and its five-test target passed again.
- Live run 01M3M7ESV21WVQHEG356P8P1ZS is still on fourth flow-generation attempt
  after successive binding parse failures (missing source, string source=file, missing
  source type). Existing daemon PID99847 does not yet include this turn's prompt
  context/reference fixes. Do not restart until that run reaches terminal or is stopped
  deliberately. Build handle65653 is producing updated daemon/CLI.
- Further generator issue: profile catalog lists outcomes/runtime but not required
  bindings or produced artifact contracts; generator cannot infer exact spec inputs.
  Required spec bindings are currently rendered leniently empty. Address before claiming
  an executable, complete app-creation workflow.
- Build handle65653 completed successfully; updated binaries are on disk but not yet deployed. Live run reached StageToolReceipt/ArtifactProduced/OutcomeReported at seq74-76; cleanup/post-processing still pending at last observation.
- Live fork reached PipelineMaterialized/flow_gate seq83. Inspected output: RunArtifact
  now correct, but generator removed all bindings and put task_specification and
  verification_specification into unused custom_fields. Stopped deliberately before
  approval. No active run remains from this test; safe to deploy new binaries.

### Required profile input gate — 2026-09-28

- Profile catalog now exposes input names, required/optional status and declared
  source roles. Generator prompt explicitly requires real bindings, not custom_fields.
- New profile-input validation runs before bootstrap PipelineMaterialized, at engine
  admission, and before agent session opening. Rejects absent/duplicate targets,
  optional required targets and empty static required inputs; runtime also rejects
  empty resolved input content. Graph validation rejects missing NodeOutput producers
  and unsupported GlobPattern bindings.
- Regression verifies failed required inputs emit BootstrapEditRequested and never
  PipelineMaterialized. 351 library tests and nine profile registry tests pass.
- Scope: this proves required-input presence/resolution, not ExpectedBindingSource
  role enforcement or producer artifact-contract matching. Static specs are explicitly
  permitted by the new test; don't claim full dataflow validation.
- All prior live generation forks are stopped. Daemon PID99847 still runs previous
  binary. New changes need strict Clippy/build/deployment before live acceptance.
- Forking the old bootstrap preserves its old profile_catalog seed; for the next
  end-to-end proof prefer a fresh supervised operation to capture the updated catalog
  and verify automatic child continuation.
- Expanded bootstrap integration run exposed invalid existing fixtures: driver_e2e
  five tests passed, but bootstrap_multi_milestone_test timed out waiting for flow_gate
  because its golden graph has missing required bindings. This is not a passing gate.
  Keep validation strict and migrate the shipped/example contracts rather than suppress
  checks. bootstrap_validation_retry_test was not executed after prior target failed.
- Next concrete migration: bundled linear-3 still uses implementer as placeholder
  spec stage, pass/fail ports mismatching actual profile outcomes, and empty bindings.
  Replace with spec-author drafted/needs_clarification -> implementer implemented/
  blocked/partial -> verifier passed/failed; bind milestone from initial prompt,
  implementation/verification spec from spec-author output (actual artifact alias
  must be verified; spec.toml + spec.md may produce spec-toml/spec-md).
- Golden multi-milestone fixture still has fake roadmap_planner, nonexistent produced
  artifact name roadmap.milestones, empty bindings, and terminal-outcome shorthand.
  Migrate to run_artifact roadmap seed, explicit body terminals, per-task spec author
  and real spec bindings. Update topology assertion to the RunArtifact form.
- Do not deploy or claim broad regression pass until these relevant integrations
  are green. No live model run is active.

### Executable template migration — 2026-09-28

- Corrected prior diagnosis: bootstrap fixture timeout was initially at
  description_gate, because its run-only resolve_human_input(None) call is now
  rejected with MissingGateRequestIdentity. Test helper now uses node/call identity
  from the durable HumanInputRequested event and prints recent events on timeout.
- Migrated bundled linear-3 to actual spec-author@1.0, implementer@2.0 and verifier@2.0
  outcomes and bindings. Migrated multi-milestone and its golden fixture to seeded
  roadmap, nested task specification/implementation/verification, explicit terminals,
  bounded repair edges and a separate whole-roadmap final specification/verification.
  Kept diagram positions. Verified all three local profiles route to codex-acp.
- Actual duplicate-stem spec output alias is spec_toml, not spec-toml. Templates
  use the emitted underscore alias. NodeOutput now takes the latest matching
  producer artifact instead of the oldest; regression uses distinct old/new files.
- 352 orchestrator library tests pass; three five-test bootstrap targets passed
  after helper migration. Eight bundled-flow core tests pass after updating exact
  warning expectations: multi-milestone now has same-runtime final verification
  warning but no longer only one verification boundary.
- Strict core/orchestrator Clippy handle32777 and expanded bootstrap test handle
  from this turn are still pending at this record. New templates/enforcement/context
  need rebuilt binaries and deployment; no live model run is active.

### Deployment and fresh supervised acceptance — 2026-09-28

- Expanded five bootstrap integration targets passed (25 tests). Strict core and
  orchestrator all-target Clippy passed. Daemon, CLI and UI build succeeded.
- Restarted daemon: PID 8691, ping OK. It now includes required-input checks,
  current iteration context, latest node-output selection and migrated templates.
  Preview app bundle is still older; built target/debug/surge-ui is current.
- Old demo repository was rejected because of untracked planning drafts, correctly
  preserving the clean-base admission rule. Cloned its committed base to
  /private/tmp/surge-timer-supervised-20260928 and copied its ignored surge.toml
  (all six planning/spec/implementation/verification profiles resolve codex-acp).
- Fresh durable bootstrap accepted: operation 01M3M98EJAWCBW5AQBXHTRG0B1,
  planning run 01M3M99AMV6QKCNRKBR5GEJ8QA, implementation reserved
  01M3M99AMVVW1X8Y1508MPFD2B. Latest response pending/queued_planning revision0.
  Request/status saved /tmp/surge-fresh-bootstrap-operation.json. Next: inspect
  progress, review exact generated artifact at each approval, then prove automatic
  implementation continuation and run generated app tests/browser checks.
- Full application creation is not yet proved. Do not restart daemon during this
  acceptance run or change pinned profile/config inputs while it is active.

- Fresh operation actually reached needs_attention/preparing_planning with
  partial_startup, revision2, before an event store/model run was created.
  Found bootstrap flow marks profile_catalog binding optional=true while the
  actual bundled/local flow-generator requires it. Removed optional flag and
  extended bundled required-input regression to bootstrap. Targeted test pending.
  This last fix is NOT in running PID8691; rebuild/restart required. Captured
  operation pins old bootstrap graph, so cancel it and submit a fresh operation
  after deployment rather than retrying changed captured inputs.

### Supervised execution diagnosis — 2026-09-28

- Previous turn was progress: bootstrap required catalog binding fixed; targeted
  bundled validation passed. CLI/daemon rebuilt and restarted as PID9774.
- Operation01M3M98EJAWCBW5AQBXHTRG0B1 cancelled successfully (revision4).
- Fresh operation01M3M9KXEYNVZNWDQX4656E1BG accepted and really launched
  planning01M3M9KXFTD3N3F0SAWCVJVMJ4 in managed worktree. Reserved child
  01M3M9KXFTBEXKFD0JD2F7P6F0. It failed at seq9: prompt completed without
  accepted valid outcome candidate. Codex wrote a valid-looking description.md
  but explicitly reported report_stage_outcome absent from callable tools.
  Rollout: ~/.codex/sessions/2026/09/28/rollout-2026-09-28T10-20-00-01a0e899-fc88-73a1-a6eb-bebd76cbbc10.jsonl
  (read only public message/tool items, never reasoning). No active model run.
- Stage MCP descriptor is prepared unconditionally at agent.rs662; helper_path
  chooses sibling surge, daemon current executable is target/debug/surge-daemon;
  fresh sibling surge was built. Worker new_session passes stdio stage descriptor.
  No agent stderr/mcp failure in daemon log. Need diagnose why actual Codex session
  lacks tool, do not treat plain text intended outcome as successful completion.
- Test harness now supplies a registry/catalog. Its three planning profiles retain
  real input contracts but explicitly strip shell hooks/output artifact contracts
  for scripted minimal document routing fixtures (these are not artifact acceptance
  tests). Four bootstrap targets pass20tests; strict targeted Clippy passes.
- bootstrap_archetypes_test still fails: bundled bug-fix implement_1 missing spec
  binding. Old bug-fix/refactor/spike templates still placeholder implementer1 with
  empty bindings. Migrate properly; do not weaken runtime guards or claim all green.
- Native UI reconnect succeeded DAEMON LIVE. Opened project
  /private/tmp/surge-timer-supervised-20260928 via chooser, but UI showed0runs even
  around this supervised operation. Investigate global supervised discovery/history
  alongside missing outcome tool. CUA binding surgeApproval currently points to
  /tmp/SurgePreview.app; preview bundle still older than target/debug binaries.

### Stage MCP helper startup fixed — 2026-09-28

- Reproduced missing Codex outcome tool: `surge internal-stage-mcp` ran CLI
  orphan housekeeping first. In actual managed worktree stdout contained
  `Found 2 orphaned worktrees. Clean up? [Y/n]`, corrupting MCP protocol and
  consuming initialize as a cleanup answer. Excluded InternalStageMcp from
  interactive orphan scan in CLI main. Rebuilt CLI; same-worktree repro now
  has empty stdout (missing-auth intentional exit1). No daemon restart needed.
- Added cli_stage_mcp_startup regression with a real orphan branch, protocol input,
  timeout, empty stdout and preserved branch assertion. Test passes; strict CLI
  test Clippy passes. git2 added as workspace-managed CLI dev dependency.
- Fresh operation01M3MA24BRPEXHYYX9KJD1H0D9 now really works through description:
  planning01M3MA24C62ZVG8ZMRDAFBP1SC; reserved child01M3MA24C6Z8FVPS8WBJJGV6A3.
  Received StageToolReceipt/OutcomeReported and description gate seq16. Reviewed
  description.md and approved exact request gate-01M3MA3X45VPJK53BYY2AK8X9F
  through ResolveGateInput successfully. Roadmap is next. ACTIVE live run: do
  not restart daemon or change pinned profile inputs. Operation request/status
  file /tmp/surge-fresh-bootstrap-operation.json now identifies this operation.
- Found UI discovery omission: supervisor never published global RunAccepted.
  Added publication after successful start/resume before tracked forwarding;
  existing live planning gate integration now asserts global discovery. Test passes.
  This daemon change is NOT deployed (PID9774 still running). Strict lib Clippy
  handle42998 pending at record time. Preserve live run before rebuild/deploy.
- Broader remaining work: old bug-fix/refactor/spike templates still invalid
  required bindings (bootstrap_archetypes integration red); full app implementation
  and native/browser verification not yet proved.

### Roadmap approval and inherited catalog — 2026-09-28

- Live planning01M3MA24C62ZVG8ZMRDAFBP1SC produced validated roadmap.toml
  schema2, with UI/accessibility, timestamp timer/state/validation, offline tests
  and README milestones. Reviewed actual content and approved exact gate
  gate-01M3MA8R109YCKKXD7WN9B172Z. Approval asks one linear3 chain to cover
  all roadmap tasks; no restart/recapture. Flow generation is next/current.
- Fixed profile catalog to render resolved inherited profile fields (bindings,
  outcomes, sandbox, authority), rather than raw leaf fields. Three implementer
  specialization rows previously hid inherited required spec input. Added
  explicit regression; all6catalog tests pass. Clippy handle99282 pending.
- Catalog fix NOT deployed: daemon PID9774 and active run retain prior catalog.
  Do not change pinned profiles/config or restart during active acceptance.
- Prior supervisor RunAccepted fix strict Clippy completed successfully. It
  also remains undeployed until safe restart.

### Automatic implementation continuation proved — 2026-09-28

- All108daemon library tests pass; pending catalog Clippy99282 completed clean.
- Flow generator produced valid linear3 graph on its first candidate this run:
  spec_app spec-author1 binds milestone=run_artifact roadmap and description seed;
  impl_app implementer2 and verify_app verifier2 bind NodeOutput spec_app/spec_toml.
  Correct actual outcomes, explicit success/failure terminals, bounded3repair edges.
- Reviewed actual flow and approved exact gate-01M3MABWN9DC3K0HD5FCWWMS2R.
- WITHOUT manual child start, durable supervisor advanced operation
  01M3MA24BRPEXHYYX9KJD1H0D9 to pending/implementing revision5 and started
  child01M3MA24C6Z8FVPS8WBJJGV6A3. Child eventstore has seeded artifacts,
  StageEntered seq8 and SessionOpened seq9 (spec_app). Managed child worktree:
  /Users/vanyastafford/.surge/worktrees/run-01M3MA24C6Z8FVPS8WBJJGV6A3.
  This proves automatic planning-to-implementation continuation, not completed app.
- ACTIVE child: preserve daemonPID9774 and pinned profiles/config. Next inspect
  generated spec and subsequent implementation/verifier results; run produced app
  tests and browser/native acceptance. UI global discovery and resolved catalog
  fixes still not deployed; do so only after live operation reaches terminal.

### Specification rejection exposed missing real retry — 2026-09-28

- Child01M3MA24C6Z8FVPS8WBJJGV6A3 wrote spec.toml/spec.md, covering app
  requirements, but TOML repeats subtasks as string list and array-of-tables.
  CLI validator reproduced duplicate-key error. Spec-author prompt itself said
  spec contains subtasks before describing tables; clarified bundled prompt to
  use only [[spec.subtasks]], never a separate subtasks list. Disk profile has
  NOT been changed yet; bundled profile test pending at this record.
- Actual outcome hook rejected spec at seq11-12. Child FAILED seq15 with
  'prompt completed without an accepted, valid outcome candidate'. No live model
  run remains. Automatic child creation was proven, app implementation not reached.
- Concrete retry defect: agent.rs queues candidates until prompt success, then
  runs hooks. record_outcome_rejection only records/counts (bounded max_retries),
  never sends feedback. Branch continues; prompt_success remains true, candidates
  empty, try_recv empty -> immediate failure at line799. Thus real MCP path has
  no retry despite remaining allowance. Legacy mocks can emit later outcomes and
  mask this. Fix must schedule another ACP send_message after completed turn with
  rejection feedback and require a new accepted candidate, respecting cap/cancel.
- Suggested implementation: pending rejection feedback set in each of three
  rejection branches (hook, authority, artifact contract). At loop top after all
  candidates consumed, for nonlegacy bridge reset prompt_success/prompt_joined,
  replace mutable prompt_finished token, spawn followup send_message task. Reuse
  existing eventloop to service stage MCP while new prompt runs. Do not await
  send_message inline (deadlocks tools). Preserve cleanup/join/cancellation and
  never accept stale previous-generation candidate blindly. Needs real stage-MCP
  integration regression, not only legacy MockBridge.
- CLI artifact validate absolute argument also reports invalid_artifact_path,
  though actual hook runs relative spec.toml; don't confuse that CLI usability
  issue with duplicate TOML rejection in live stage.

### Real ACP rejection retry implemented — 2026-09-28

- agent.rs now schedules a fresh asynchronous send_message after a completed
  turn's candidates all fail validation. Feedback includes rejecting hook/contract
  reason and asks for a new unique call_id. Resets prompt task/join/success and
  cancellation signal; same eventloop services MCP during repair. Existing
  rejection counter bounds retries; operator steer delivery is not duplicated.
- Real mock ACP peer supports retry/always-fail cases, checks feedback reached
  second prompt and reports unique call IDs. Added watchdog integrations: repair
  succeeds with two receipts/one rejection/one accepted outcome; always-fail stops
  at two rejections with max_retries1. All16engine_acp_permission tests and
  all7on_outcome_retry tests pass. Strict targeted ACP/orchestrator Clippy passes.
- Hook rejection diagnostics now include bounded stdout as well as stderr; actual
  artifact CLI emits parse details on stdout. Regression covers details and UTF8
  truncation; all13hook lib tests pass. Followup strict lib Clippy pending.
- Bundled spec profile validation15tests passed. Also updated local spec-author
  prompt with exact same subtasks clarification (retains codex runtime). All
  previous live runs terminal; no active model operation.
- Build9158 pending for CLI/daemon/UI; strict Clippy handle from latestcall pending.
  Running PID9774 still old. Need finish build, restart, deploy Preview bundle,
  and fresh supervised acceptance (old capture pins prior local profile).
  Full app implementation still not reached; old archetype test issue remains.

### Updated runtime and native discovery verified — 2026-09-28

- Build9158 and strict Clippy50316 completed successfully. Confirmed previous
  operation terminal failed revision6, then restarted daemon: PID18379 ready.
- Deployed latest surge-ui/surge/surge-daemon into /tmp/SurgePreview.app, closed
  old app (window close did not terminate; SIGTERM known preview PID94276),
  opened fresh app and selected surge-timer-supervised-20260928.
- New supervised operation01M3MAZF0PBJ51EFDG40G15N8N accepted. Planning
  01M3MAZF11PG9MJXMVJJ8MPN13; reserved implementation01M3MAZF11WCCMWC1F7A67WFEF.
  Latest request saved /tmp/surge-fresh-bootstrap-operation.json.
- Native UI observed transition from LIVE0runs0active to LIVE1run1active without
  reopening: global RunAccepted discovery fix is now proven in native UI.
  CUA binding freshSurge, Fleet view; Inbox currently31 (refresh indices).
- Runtime now includes rejected-outcome repair turn, bounded validator stdout
  feedback, inherited catalog and supervisor discovery. Local spec-author prompt
  clarification loaded. ACTIVE new run: do not restart or mutate pinned profiles.
  Next: review/approve exact docs as generated, verify spec correction/validity,
  implement and test actual app. No full end-to-end app success yet.

### Native Inbox approval confirmed — 2026-09-28

- Active planning01M3MAZF11PG9MJXMVJJ8MPN13 reached description_gate seq16.
  Read actual description.md, requirements preserved. Native Inbox renders
  markdown document rather than JSON schema. Full document is scrollable.
- Initial AX click Approve returned offscreen (document long); scrolling right
  detail panel down2pages exposed response/actions. setValue appeared ineffective
  (comment remained blank), so approved without optional comment after review.
  Native click Approve succeeded; Inbox now Nothing blocked on you. This proves
  actual native approval delivery for current runtime, not just API simulation.
- Screenshot observed list rendering sometimes puts bullet on separate line;
  readable but typography needs later cleanup. Decision actions below document
  require scrolling; consider sticky footer after core acceptance complete.
- ACTIVE operation01M3MAZF0PBJ51EFDG40G15N8N continues to roadmap; preserve
  daemonPID18379 and pinned inputs. CUA freshSurge remains open in Inbox.

### Roadmap scope repair requested — 2026-09-28

- Active planner produced roadmap, then self-refined it before outcome to one
  milestone, but retained t1 Draft app specification plus profile binding/outcome
  criteria in product tasks. Reviewed final actual roadmap before deciding.
- At roadmap_gate seq35 requested EDIT using exact gate
  gate-01M3MB8RM0YRYJ7ZJS7A931DFH. Feedback removes recursive planning and
  profile configuration tasks, preserves all timer/test/README requirements,
  repairs dependencies, retains Codex/linear3 as constraints. Resolve accepted.
- Added bundled roadmap-planner instruction distinguishing execution constraints
  from product tasks; all15bundled profile tests pass. This prompt edit is NOT
  deployed and local pinned profile untouched while operation remains active.
- ACTIVE planning01M3MAZF11PG9MJXMVJJ8MPN13 will revise roadmap. Keep
  daemonPID18379. Next review revision and approve only actual app delivery scope.

### Revised app roadmap approved — 2026-09-28

- Live roadmap edit worked: planner removed recursive specification/configuration
  tasks and rewrote seven app-delivery tasks. Read whole revised roadmap.toml;
  UI/timestamps/control validation/responsive accessibility/offline behavior and
  accessibility tests/README retained, task references consistent.
- Approved exact new request gate-01M3MBDYWHJHJAPSHDBV44V3JY at seq55 for
  planning01M3MAZF11PG9MJXMVJJ8MPN13. Requested single linear3 Codex-only
  implementation covering complete roadmap. Operation remains active; next flow.
- No daemon restart or changes to pinned profiles. PID18379.

### Revised roadmap reached implementation — 2026-09-28

- Flow generation completed without validation retry after roadmap edit. Reviewed
  flow.toml: spec-author1 reads approved roadmap/description; implementer2 and
  verifier2 bind spec/spec_toml; correct outcomes, explicit terminals, bounded3
  repair edges, verified success path. Approved gate-01M3MBHG3XZ87X0K86FVMDA0AE
  at planning seq72 through exact ResolveGateInput.
- Supervisor automatically started child01M3MAZF11WCCMWC1F7A67WFEF; child
  eventstore contains StageEntered8/SessionOpened9. Native UI now LIVE2runs1active
  without reopening, proving supervised child discovery too.
- ACTIVE child worktree ~/.surge/worktrees/run-01M3MAZF11WCCMWC1F7A67WFEF.
  Next inspect spec validation and follow implementation/verifier; keepPID18379.
- Updated docs/vibe-coding-harness.md to distinguish historical source gaps from
  verified runtime evidence and still-open full-app acceptance. No competitive
  superiority or finished-app claim.

### Live validation repair and app implementation reached — 2026-09-28

- Child01M3MAZF11WCCMWC1F7A67WFEF produced spec files. First read happened
  during writing (empty file); do not count that premature CLI read as final
  validation. Durable hooks then passed spec.toml but rejected spec.md seq12-13.
- WITHOUT operator editing artifacts, runtime sent repair feedback. Codex made
  second MCP call offline-countdown-timer-spec-drafted-002, fixed flat checkbox
  criteria, and both hooks passed seq15-16. spec_toml/spec_md captured17-18;
  OutcomeReported drafted19. Independent CLI validation both files now OK.
  This is live Codex proof of the real bounded-retry fix, not only mock evidence.
- Read full spec.toml: seven app tasks preserve timer UI/state/validation/elapsed
  time/accessibility/responsiveness/tests/README. No recursive planning tasks.
- Child progressed through SessionClosed20/StageCompleted22 into implementation
  StageEntered23 and SessionOpened24. ACTIVE implementer; do not restart daemon
  or change pinned profiles. Next inspect produced app/test evidence, then verifier
  and native/browser result. DaemonPID18379; freshSurge UI open.

### Explicit stage completion protocol — 2026-09-28

- Latest child01M3MAZF11WCCMWC1F7A67WFEF FAILED at implement: Codex
  created index.html/styles.css/app.js/tests/timer.test.js/README.md, independently
  ran Node tests (8 pass), but ended with prose ready_for_verification and NO
  report_stage_outcome receipt. Verifier never ran. Artifacts preserved in its
  managed worktree; do not claim completed app or completed operation.
- Engine now appends a uniform completion protocol to both system and initial
  prompt with actual declared outcomes, required tool submission, revised call ID,
  and candidate-vs-verification distinction. Unit regression passes; all16 real
  ACP subprocess tests pass. Actual Codex behavior with new prompt still unverified.
- Markdown list markers now have a separate non-shrinking column, preventing
  orphan markers when long list text wraps. Two unit tests pass. Moved tests to
  file end after strict Clippy caught items_after_test_module.
- Built CLI/daemon/UI, copied to /private/tmp/SurgePreview.app. Graceful shutdown
  took time (socket removed before process exit); started replacement PID26115.
  Native project opened and shows DAEMON LIVE. No new operation started yet.
- Next: live completion contract acceptance through verifier; browser test of
  produced app; native long-list visual check; preserve known archetype test debt.

### Browser acceptance falsified Node-only timer evidence — 2026-09-28

- Previous turn was progress: completion contract, tested ACP path, deployed
  binaries, live daemon. Revalidated PID26115 ping OK.
- Started operation01M3MC9ZYW75AY10QP5HKKSPSF after that fix; planning
  run01M3MC9ZZ6F56E1GRB9ACER284, reserved child01M3MC9ZZ6AP0R1RVE2DH2AWR2.
  Request/status saved /tmp/surge-completion-protocol-operation.json.
- Browser-served prior child app at127.0.0.1:8765 (exec61016). Actual keyboard
  Start/Pause/Resume updates states, but countdown stalls. Browser error log:
  TypeError Illegal invocation at app.js144(scheduleTick), through start81 /
  resume103. Native setTimeout stored on CountdownTimer called with wrong receiver.
  Eight Node tests with injected scheduling did NOT cover this runtime behavior.
  Do not certify prior app. Its files were not modified.
- Native updated Inbox displays full description and corrected non-orphan list
  markers (screenshot). Pasted browser acceptance/known regression into Decision
  response and clicked Request changes. Exact full comment persisted in
  HumanInputResolved seq17, gate01M3MCBEK2BSQEDZT4P52WK33T; edit acknowledged
  seq19, new description agent SessionOpened24. New run remains active.
- CUA handles freshSurge, timerTab, timerBrowser, timerDiagnostic(tab1); tab at
  localhost. Browser console evidence read through CUA dev.logs.
- Strict Clippy for orchestrator and UI all-targets passed after test relocation.

### Remaining archetype input contracts repaired — 2026-09-28

- Previous turn was progress: real browser failure evidence and durable Inbox
  edit. Revalidated planningrun01M3MC9ZZ6F56E1GRB9ACER284: revised description
  explicitly requires actual-browser acceptance. Approved gate01M3MCEWPQDB21PFRJQGSM9166.
- Roadmap agent still added linear-3-flow-contract product milestone despite
  execution-only constraint. Requested removal at gate01M3MCMHHFGVY3Z5BMSBBFAD1Q;
  preserve all actual app/tests/browser/README work. Active roadmap revision
  SessionOpened61. Do not restart daemon26115 or re-start this operation.
- Reproduced bootstrap_archetypes_test failure: implementer@1.0 required spec
  missing, not an unexplained gate timeout. Migrated bug-fix/refactor/spike
  bundled flows with spec-author input from initial prompt; implementer2 spec
  input from spec_toml; declared profile outcomes; explicit blocked terminals
  and bounded partial/verification/review retries.
- Bug-fix preserves reproduction stage and reproduction.md handoff before patch.
  Refactor preserves characterization baseline, passes baseline.md to implementer,
  requires changes.patch including new files, passes changes to reviewer after
  sealed verifier. Spike performs bounded experiment with findings.md and makes
  no verified-ledger claim. All agent stages have explicit runtime limits.
- Fixed missing edge_kind_hint found by first parse test. Five bootstrap archetype
  tests now pass;25 core bundled-related tests pass. Final timeout additions are
  straightforward TOML but should rerun these checks before deployment.
- New templates are source-only, NOT deployed into active daemon. Existing live
  operation still uses prior pinned linear-3 reference. Full live pipeline remains
  unverified; prior generated timer remains faulty and preserved for regression.

### Approved focused roadmap; archetype examples synchronized — 2026-09-28

- Revalidated prior pending test96522: all5 bootstrap archetype tests pass after
  final runtime limits. Previous turn was concrete progress (three repaired flows).
- Reviewed complete revised roadmap at planningrun01M3MC9ZZ6F56E1GRB9ACER284.
  Only timer-experience and delivery-evidence milestones remain; actual browser
  acceptance preserved. Approved gate01M3MCR1PJW5HH2FYPR4SXEGXS with instruction
  that one spec covers the complete roadmap. Current flow_generator SessionOpened80.
  Operation01M3MC9ZYW75AY10QP5HKKSPSF remains active; no restart or duplicate run.
- Synchronized examples/flow_bug_fix.toml, flow_refactor.toml, flow_spike.toml
  with newly repaired bundled templates. Updated docs/bootstrap.md and affected
  gallery sections/diagrams to describe actual spec and artifact handoffs, bounded
  repair, blocked exits, and experiment success without verification authority.
- Scoped git diff --check passes. CLI examples_smoke test handle64974 started;
  collect result before claiming examples fully verified. New templates remain
  source-only; daemon26115 stays running the pinned live operation.

- CLI example smoke initially found its resolver only allowed legacy implementer1
  and planner1. Changed resolver to consult actual BundledRegistry role/version
  entries plus legacy mock planner (not a blanket allow-all). First compile found
  re-export path typo; corrected to surge_core::BundledRegistry. Test25102 and
  Clippy42377 are pending; collect both.
- Reviewed generated complete flow and approved gate01M3MCW48GPKCPJ178CMGDMV3W.
  Supervisor automatically launched child01M3MC9ZZ6AP0R1RVE2DH2AWR2; durable
  StageEntered8/SessionOpened9 confirms spec stage live. Parent planning finished.
  Next follow child spec/implementation/completion-tool/verifier, no restart.

### Direct input-contract regression covers repaired archetypes — 2026-09-28

- Collected final CLI examples tests25102:10 pass. Strict Clippy42377 passed.
  Previous turn was progress: synchronized examples/docs, approved full flow,
  automatic child launch.
- Expanded real ProfileRegistry input-contract regression in engine/validate.rs
  to bug-fix/refactor/spike in addition to bootstrap/linear-3/multi-milestone.
  Test98586 passes; catches missing spec bindings directly instead of gate timeout.
- Revalidated child01M3MC9ZZ6AP0R1RVE2DH2AWR2 at SessionOpened9(spec_app).
  ACP process31568 live; public rollout11-17-28-01a0e8ce-981c-7421-ac7d-a7dc4d1206bb
  reports artifacts written and structural checks at16:19:33Z. Earlier zero-byte
  files were observed mid-write, not an accepted output. No restart.
- Next: await actual stage receipt/hook results and implementation. Browser defect
  from previous app remains documented; do not claim Node tests prove UI behavior.

- Child receipt10 submitted actual drafted candidate; TOML hook11 passed,
  Markdown hook12 rejected, OutcomeRejectedByHook13. Automatic repair active.
  Found bundled spec-author prompt asked for criteria grouped by subtask while
  validator expects flat checkboxes. Clarified source prompt to flat checkboxes
  prefixed by task IDs, no subheadings/nested lists. Source-only; no live profile
  mutation or restart. Validate bundled profile parsing in next checks.

### Spec repaired; implementer running with completion contract — 2026-09-28

- Prior turn progress: required-input regression plus spec-author prompt fix.
- Child01M3MC9ZZ6AP0R1RVE2DH2AWR2 repaired spec automatically and progressed
  to StageCompleted22, StageEntered23, SessionOpened24(impl_app).
- Inspected accepted spec.toml: actual browser countdown/control/keyboard/console
  criteria preserved; unavailable browser evidence must be explicitly incomplete.
- Updated bundled spec-author parses:15 profile tests pass (98781). Scoped
  git diff --check passes. Full orchestrator library tests82437 running; collect.
- Implementer public rollout11-20-51-01a0e8d1-b1e8-7a21-8d37-c970066451e7
  growing at11:21 local. No duplicate run or daemon restart. Next actual outcome
  receipt and verifier transition remain unproven; keep watching same child.

- Full library suite82437 found only the intentional roadmap prompt snapshot
  change (354/355 pass). Reviewed one-line execution-constraints diff and accepted
  that single snapshot. Rerun40501 started; collect its authoritative result.

### Bootstrap integration regressions green — 2026-09-28

- Confirmed full library rerun40501:355 pass. This turn revalidated child24
  SessionOpened impl_app and live ACP32964. Public rollout continues implementation
  after reading accepted spec; no outcome yet. Same operation, no restart.
- Four bootstrap targets (linear_3, multi_milestone, validation_retry, edit_cap),
  test16211:20 total pass. Initial command had incorrect linear target name;
  corrected from actual file inventory before execution.
- Full git diff --check found only trailing blank lines in earlier modified
  linear/multi flow, golden fixture and Markdown renderer; normalized EOFs.
  Full diff check now clean. No other preexisting edits overwritten.
- Still need actual implementer tool submission, sealed verifier evidence and
  browser run. New templates/profile prompt updates remain undeployed while
  live operation is running.

### Real browser checks on current app — 2026-09-28

- Child01M3MC9ZZ6AP0R1RVE2DH2AWR2 produced index.html/app.js/styles.css/tests.
  Still impl_app SessionOpened24; no accepted outcome yet.
- Served current child at127.0.0.1:8766, exec51689 (old faulty app server61016
  on8765 remains separate). CUA currentTimer tab2/currentTimerDiagnostic.
- Independently ran node tests/timer.test.js:6 pass. Actual browser:8second
  countdown completed00:00;30second run paused27,resumed26, reset30; seconds60
  rejected with focus to seconds; keyboard corrected4,Tab→Start,Enter→running,
  Tab→Pause; console error list empty. Mobile390x844 screenshot fully fits,
  document scrollWidth/clientWidth390; viewport reset. Tab marked handoff.
- Wrote operator-browser-evidence.json in child worktree with exact source
  SHA256s and limited observations; no blanket acceptance claim. Sent durable
  boundary steer6HK5MAJ8AQ directing next agent to check hashes and remaining
  gaps. README currently marks browser verification incomplete. No code or
  README modified by operator; six tests and browser evidence are independent.
- Follow completion tool receipt, SteerDelivered, sealed verifier. User goal
  remains active; do not claim full app/cycle or competitive completion yet.

- LIVE COMPLETION CONTRACT VERIFIED: implementer called report_stage_outcome
  receipt25, artifacts26-30 captured, ready_for_verification31 accepted,
  StageCompleted34, verifier StageEntered35/SessionOpened36. Source hashes match
  independent browser evidence. Implementer also claims own Chrome tests and
  updated README in final artifact; inspect that evidence rather than assume.
  Verifier currently active; no completion claim yet.

### First complete live Codex app-factory operation — 2026-09-28

- Operation01M3MC9ZYW75AY10QP5HKKSPSF bootstrap_status confirms completed,
  revision6, implementation01M3MC9ZZ6AP0R1RVE2DH2AWR2, terminal end.
  Child receipt39 repaired missing schema_version in verification-report;
  artifact40 hashc4e59f03e5152611c9b393c2d709a66753afce046803aaf3d57d93a286a25d62,
  passed41, StageCompleted44, RunCompleted46. Independent artifact validation OK.
- Implementer public rollout proves actual Chrome DevTools calls (generic names
  emulate/evaluate_script/list_console_messages); README browser claims have
  tool evidence, not only model prose. Verifier independently ran Node tests,
  JS syntax and source/spec/documentation inspections; read operator evidence.
  Browser checks are implementer+operator evidence, not verifier's own browser run.
- App JS/CSS/HTML hashes still exactly match operator browser checks after verifier.
  App at child managed worktree; preview127.0.0.1:8766 running via exec51689.
- Verifier source prompt omitted required schema_version; added explicit top-level
  schema_version=1. Profile tests5096 pending. Do not mutate readonly policy
  just to make reporting work; sandbox enforcement itself still needs audit.
- Important remaining scope: comparative acceptance benchmarks, recovery/fork
  worktree correctness, reliable reusable previews, evidence coverage and sealed
  verifier enforcement, final UI/performance/release gates. One completed timer
  proves this vertical slice, not whole goal or superiority over competitors.

### Completed operation survives real daemon restart — 2026-09-28

- Previous turn progress: completed live operation and independent verification
  evidence,15 updated verifier-profile tests pass.
- Verified daemon hosted no active runs before shutdown. Stopped26115 gracefully;
  restarted same binary as36390, ping OK. bootstrap_status still completed with
  identical operation/planning/child IDs, revision6, terminal end. Hosted run list
  empty after recovery; immutable settled child database still RunCompleted46.
- sqlite3 -readonly initially failed after WAL cleanup despite database present;
  opened completed DB mode=ro&immutable=1 successfully. Do not use immutable for
  active WAL writers. No data loss or run restart inferred from transient reader error.
- bootstrap_owned_recovery+bootstrap_operation_ipc tests31943:3 pass. These cover
  reserved-ID routing/refusal, not full active-run crash recovery. Real completed
  restart evidence does not satisfy restart-during-work acceptance by itself.
- Diagnosed slow idle shutdown: lifecycle::drain unconditionally sleeps full
  grace after token cancellation, main aborts remaining supervisor after that.
  Improve only with tracked drain completion, retaining bounded shutdown and
  final-event delivery; do not just delete sleep or reduce safeguards.
- Fork remains known gap: engine/fork copies parent project_path and explicitly
  expects caller to provision a fresh worktree; CLI fork currently only calls
  storage fork and advertises resume. Need complete owned worktree lifecycle.
- New templates/prompt fixes still not deployed; restart used prior binary.
  Completed app preview server51689 remains at8766. Broader goal stays active.

### Shutdown ownership and bounded early drain — 2026-09-28

- Prior turn real completed-run restart evidence was progress. Inspected current
  shutdown: unconditional grace sleep, detached IPC handlers.
- server run_host now owns connection handlers in JoinSet, reaps completed tasks,
  joins after listener shutdown. Outer deadline still cancels remaining handlers
  by dropping server task/JoinSet. Main waits for server+bootstrap finished, zero
  admission reservations, zero broadcast run forwarders before early exit.
  Full grace deadline retained for active/incomplete work.
- Added lifecycle::drain_until plus tests for fast settled exit, bounded unsettled
  exit, no early exit before signal.111 daemon lib tests67008 pass.
- Bin check53909 pending; strict all-target Clippy pending (see tool handle).
  Changes source-only, daemon36390 unchanged. Must inspect test results and run
  real idle/recovery validation after building, before claiming timing improvement.

### Measured fast idle daemon shutdown — 2026-09-28

- Collected bin check53909 and strict all-target Clippy87377:pass. Built new
  daemon82979 successfully. Prior turn was concrete shutdown ownership progress.
- Isolated real process test with separate SURGE_HOME,30s configured grace,
  idle socket held open: Shutdown IPC acknowledged, exit0 after0.024s. Evidence
  /var/folders/5h/kmnftr3s6r1d30h8rtdll_2m0000gn/T/surge-shutdown-check-28c7gxsq/result.json.
- Added permanent Unix integration test crates/surge-daemon/tests/idle_shutdown.rs
  using CARGO_BIN_EXE daemon and owned child cleanup, bounded startup/exit waits,
  checks shutdown ACK and clean exit while idle connection remains open.
  Test11093 passes; strict targeted Clippy passes. Full diff whitespace clean.
- Main daemon36390 remains OLD running process; target/debug/surge-daemon now
  new build. Deploy/restart only after checking no live work; then verify same
  completed operation remains intact. Active-work drain/recovery still requires
  stronger testing;24ms is idle-case measurement, not a general shutdown claim.

### Deployed tested daemon and corrected local prompts — 2026-09-28

- Prior turn progress:24ms real idle shutdown and permanent process regression.
- Checked no hosted active runs; gracefully stopped old36390, waited actual exit,
  copied new daemon into SurgePreview bundle, started new primary39948.
- Updated only three exact prompt sections in local ~/.surge/profiles files:
  spec-author flat criteria, verifier report schema_version, roadmap execution
  constraints. Kept codex-acp runtime, backed originals up under
  /var/folders/5h/kmnftr3s6r1d30h8rtdll_2m0000gn/T/surge-profile-prompt-backup-27or4m8g.
- New daemon ping OK; completed operation01M3MC9ZYW75AY10QP5HKKSPSF still
  completed with identical IDs/revision6. No duplicate operation created.
- daemon_persisted_gate and daemon_resume_stream tests35821:8 pass. These
  complement prior111 library tests, idle process test and strict Clippy.
- Next actual active-run restart gate scenario or fork lifecycle fix; preserve
  all generated app artifacts. Whole comparative product goal remains unproven.

### Active gate crash reproduction found real worktree-path defect — 2026-09-28

- Previous deployment turn progress. Ran isolated real daemon + CLI human gate
  flow via /tmp/surge-active-restart-check.py (no models). Killed only that owned
  test daemon after HumanInputRequested6, restarted same SURGE_HOME. Recovery
  incorrectly appended RunFailed7(worktree lost) while custom folder existed.
  Evidence:/var/folders/5h/kmnftr3s6r1d30h8rtdll_2m0000gn/T/surge-active-restart-z8tp6j_a/evidence.json.
- Root cause recovery.rs reconstructs worktrees_root/run-ID despite engine
  persisting actual execution directory as registry.project_path. Replaced guess
  with persisted path and is_dir. No fallback to a different existing checkout.
- Updated recovery fixtures to persist actual directories; custom-checkout
  regression asserts chosen path outside conventional root.26 recovery tests
 44233 pass. Build9645 pending; MUST rerun isolated crash script after build,
  then approve recovered exact gate to completion, add durable regression test.
- Primary daemon39948 not restarted/affected; successful app preserved.
  Worktree-path change remains source-only pending live verification.

### Real crash recovery and stale-gate rejection verified — 2026-09-28

- Revalidated build9645 finished. First post-fix actual kill/restart now restores
  gate rather than marking WorktreeLost (test root surge-active-restart-m82vdd2c).
- Extended /tmp/surge-active-restart-check.py to reject old exact request, approve
  refreshed request, and await RunCompleted. Full isolated real process run passes:
 01M3MEARMDXJX5CZE8W3KTS4DA, evidence root
 /var/folders/5h/kmnftr3s6r1d30h8rtdll_2m0000gn/T/surge-active-restart-zfg_5w6z.
- Before kill:one RunStarted, HumanInputRequested6. After restart:new gate identity
  requested9; old approval returned gate no longer pending; fresh approval accepted,
  exactly one HumanInputResolved10 and RunCompleted16. No duplicated RunStarted.
- This proves custom-worktree raw gate-run crash recovery, not mid-agent or
  supervised bootstrap crash recovery. Keep those acceptance cases separate.
- Strict daemon all-target Clippy42287 passed; full diff whitespace check clean.
  Primary daemon39948 still previous deployed binary; new recovery fix only in
  target/debug. Need permanent real process regression + deployment next.

### Permanent crash regression; deployment startup anomaly — 2026-09-28

- Added crash_gate_process.rs + fixtures/recovery-gate.toml real daemon kill/
  restart test, rejects stale gate, accepts fresh, verifies exactly-once terminal
  sequence. Test6241 passes, strict Clippy93544 passes.
- No active main runs; stopped39948; copied new daemon to Preview bundle. CLI
  start said41296 ready but subsequent socket refused, stale pid and no process.
  Do not claim that deployment was healthy. Started target/debug/surge-daemon
  directly via exec60372 (live handle), investigate CLI detachment/exit next.
- Earlier final status/doc update command failed before any file writes due to
  connection refusal; no new status confirmation for completed operation yet.
  Primary daemon path fix built; check current direct process state before action.

### Detached launch verified; primary daemon stable — 2026-09-28

- Investigated startup anomaly: start without --detached inherits terminal
  descriptors/session, so prior terminal-scoped process did not survive.
  Isolated start --detached exits CLI then responds ping; shutdown acknowledged.
  Evidence directory surge-detached-start-qu7vfyg4 under system temp.
- Checked no hosted active work, stopped direct41376, launched primary
  `surge daemon start --detached`:41654 ready, subsequent separate command ping
  OK. Completed timer operation still completed with original IDs/revision6.
- Updated docs/cli.md AFK examples and getting-started guidance to use detached
  launch and describe log location. No speculative daemon crash fix applied.
- Permanent crash_gate_process test and strict Clippy from previous turn remain
  green. New recovery binary now deployed and detached. Next focus fork-owned
  workspace lifecycle or broader supervised/mid-agent recovery, not repeat this
  completed launch check. Goal remains active.

### Fork artifact independence started — 2026-09-28

- Previous turn detached launch and documentation proof was progress. Inspected
  fork: event/snapshot copies lacked child-owned artifact bytes, relying on
  parent paths. Added copying every inherited ArtifactProduced using open_ref
  fallback and explicit content-hash verification into child's artifact store.
- Atomically reserve a fresh child directory before copying, rejecting existing
  destination IDs so their artifact indexes are never modified. Failed copy may
  leave a reserved directory but no lineage success; don't silently reuse it.
- Regression deletes only test parent artifact after fork and reads child-owned
  bytes.9 fork tests30515 pass before final reservation guard; rerun pending
  (latest tool handle), strict Clippy55914 pending. Source-only; not deployed.
- Still need full fork-owned worktree provisioning and correct persisted path,
  actual file materialization, existing/new run rejection regressions and failure
  cleanup review. Artifact copy is one part, not a completed fork fix.

### Fork artifact ownership regression checks

- Fork unit suite: 11 passed, including existing destination preservation and corrupt inherited blob rejection without parent lineage.
- Artifact byte independence is covered; full historical workspace ownership is still incomplete. Agent artifact events currently retain content-addressed paths rather than original relative filenames. Do not reconstruct historical filenames from logical names or claim that copying the current parent checkout restores the selected historical sequence.
- No daemon deployment in this step.

### Artifact store integrity on every read

- Reproduced corrupted content-addressed blobs being returned successfully by both `open` and `open_ref`: two regression tests failed before the fix. The filename alone was being trusted; only legacy fallback bytes were hashed.
- `ArtifactStore::open` now verifies bytes against the requested hash. `open_ref` propagates corruption rather than masking it with a valid fallback. `put` checks the resulting blob before publishing an index entry, including idempotent writes that encounter an existing corrupt file.
- Three regression tests cover these paths. All 368 persistence library tests pass; strict all-target Clippy for persistence and orchestrator passes. Removed the now-redundant fork-local hash check; integrity belongs at the storage boundary.
- Existing absolute artifact paths are preserved. Some loop/bootstrap consumers still open paths directly; historical workspace restoration requires explicit source-path provenance and a consumer audit.
- This change has not yet been deployed into the running daemon.

### Original artifact filenames retained in journal

- Added optional `ArtifactProduced.source_path`, separate from the existing immutable-store `path`. Real agent stage emission records the already validated worktree-relative path. Synthetic and historical artifacts may omit it; serialization omits absent values and deserialization defaults them. Existing consumers keep using the original store-path field.
- Core event compatibility test covers absent and nested source paths. Real stage emission test asserts `spec.md` / `design.md` provenance; fork test asserts `docs/spec.toml` survives prefix copying.
- Workspace/all-target check passes. Core artifact-focused tests: 44 passed. Agent artifact emission integration suite: 9 passed. Its ADR rejection fixture required the now-mandatory `spec` binding; repaired the fixture and confirmed rejection/retry reaches its intended assertions.
- This supplies provenance for future runs, not historical filenames for old content-addressed events. Full workspace reconstruction remains incomplete: declared artifacts alone do not capture every file, deletion, or Git base. No daemon deployment yet.

### Stage-boundary Git checkpoints implemented

- New `surge-git::checkpoint` captures tracked and non-ignored working files using an in-memory Git index. It does not write the user index or move HEAD. Immutable `refs/surge/checkpoints/<run>/<seq>` retain commits against garbage collection; reusing a sequence with changed bytes is rejected.
- Engine stage-boundary snapshots now carry an optional checkpoint locator (shared Git directory and commit), captured on a blocking worker. Older snapshots deserialize without this field; non-Git directories return no checkpoint.
- Tests prove additions, working edits over staged content, deletions, ignored-file exclusion, unchanged HEAD/index, and same-sequence immutability. Engine integration independently reads historical file bytes with Git CLI after modifying the live working file.
- Orchestrator library: 358 passed; snapshot integration: 7 passed. Strict all-target Clippy for Git and orchestrator passes.
- Limits: nested repositories/submodules and unresolved index conflicts are rejected; ignored untracked files are intentionally outside the snapshot. Current capture errors propagate at stage boundaries. Fork worktree provisioning and CLI wiring still pending, so full isolated fork is not yet delivered. No deployment yet.

### Isolated fork wired through engine and CLI

- `checkpoint::restore` uses pinned run-worktree preparation with recorded commit and repository identity. A regression restores earlier bytes/deletions, edits the child, proves parent isolation, and refuses to reset a modified existing child.
- `ForkRequest::with_worktree` requires an exact completed-stage snapshot with a workspace checkpoint before reserving a child. Fork restores that checkout, rewrites RunStarted.project_path, copies artifact blobs, and rebases their event paths to the child's store. History-only library forks remain available without a destination.
- CLI `engine fork` always requests an isolated checkout under SURGE_HOME/worktrees and prints its path. CLI `engine resume` now resolves the recorded run directory rather than passing the terminal CWD. Old/non-Git boundaries without checkpoints are rejected clearly.
- Tests: 4 Git checkpoint tests, 12 fork unit tests, and 7 snapshot integration tests passed. Integration covers engine snapshot → isolated fork → resume → completion and parent isolation. Strict all-target Clippy passes for Git/orchestrator/CLI. CLI binary rebuilt.
- Actual CLI proof: parent run-01M3MG0WGT658GAJCF3Y7ZZ8EH, boundary seq 7, child run-01M3MG1ZWD1ZVNF0DDHZY21D2P. Parent file changed after completion; child recovered checkpoint bytes; child edits left parent unchanged. Evidence: /var/folders/5h/kmnftr3s6r1d30h8rtdll_2m0000gn/T/surge-isolated-fork-euntdr61/evidence.json.
- Running primary daemon not redeployed yet. Remaining caveats: history-only low-level API requires caller discipline; stage capture errors currently fail boundaries; nested-frame snapshots need an audit; failure after destination reservation can leave an unregistered folder; checkpoint performance and desktop exposure are unmeasured.

### Nested resume state fixed and runtime deployed

- New integration regression forked at the first body-stage boundary of a three-item loop. Before fixes the snapshot contained zero frames; after writing frames it still failed with `cursor at unknown node body_end` because replay/resume discarded the stack.
- Stage snapshots now persist frame stacks and root traversal counters. Replay validates/deserializes both, and resume passes them into execution. Regression completes all three iterations with exactly [0,1,2] starts and three completions. Snapshot integration also checks persisted and replayed edge counters.
- Checks: static-loop target 7 passed; other snapshot/loop/subgraph targets 17 passed, 3 pre-existing placeholder tests ignored; daemon library 111 passed; strict all-target orchestrator Clippy passed. Ignored retry/skip/subgraph-branch cases remain unverified.
- Built CLI and daemon, updated bundled CLI/daemon under /private/tmp/SurgePreview.app, gracefully replaced idle primary daemon PID41654 with detached PID49672. Ping succeeds; prior app bootstrap operation 01M3MC9ZYW75AY10QP5HKKSPSF remains completed revision6 after restart. UI binary itself was not rebuilt in this step.
- Live new-daemon smoke: run-01M3MGBEMMNZ487TYETKZT60RT reached RunCompleted, boundary8 snapshot includes Git commit e6633dd07d1ddc12388667c7744bdf856c3e3afa and edge counter next=1. CLI fork created isolated run-01M3MGC0RAXXTAT97AX3F3MYT4; CLI resume invoked from the Surge repository completed it using its recorded child worktree.

### Loop failure policies now exercised end-to-end

- Replaced empty ignored retry/skip targets with real engine tests; extracted their shared three-item graph into fixtures/static_loop_graph.rs and failure runner into fixtures/loop_failure.rs.
- Reproduction: a Failure terminal inside the loop with Retry(max=2) emitted item starts [0,1,2] instead of [0,0,0]. `latest_loop_outcome` looked for an outcome on the terminal node (never emitted) and defaulted to completed, ignoring the terminal kind.
- The engine now derives loop completion from TerminalKind. Abort and exhausted Retry return typed LoopFailed after persisting LoopCompleted(aborted), preventing an outer success terminal from making a failed loop report RunCompleted. Skip records failed outcomes and proceeds as explicitly configured.
- Added real tests for two-retry exhaustion, zero retries, Abort, and Skip. Three loop targets: 19 passed, none ignored; orchestrator library: 359 passed; strict all-target Clippy passed.
- Scope limit: success after earlier failed attempts and mixed success/failure iterations still need scripted-agent integration evidence. The separate subgraph-branch placeholder remains ignored. New failure-policy fixes have not yet been deployed; primary PID49672 still has the prior nested-resume/fork update.

### Scripted failure-to-success coverage and deployed retry policies

- Restored the original intended retry/skip scenarios as real engine integrations with scripted bridge outcomes: [failed,failed,done,done,done] succeeds after two failures on item0; [done,failed,done] with Skip advances through all items and records the failed middle item. No external provider was invoked.
- Additional regression exposed retry-budget leakage across items: after item0 consumed its retries, item1 failed immediately. Confirmed RunOutcome::Failed before fixing; advancing to a new item now resets attempts_remaining from its configured failure policy. Test expects starts [0,0,0,1,1,1,2].
- Three loop integration targets: 22 passed, no ignored tests. Strict all-target orchestrator Clippy passes. Rebuilt CLI and daemon and updated bundled copies.
- Gracefully replaced idle PID49672 with detached primary PID51721; ping succeeds. Actual daemon test run-01M3MGVVXYCMGEV8QYGA8E22Q2 used a terminal-failure loop with Retry(max=2): durable starts [0,0,0], exactly one RunFailed, no RunCompleted. /tmp/surge-loop-retry-smoke.toml is the input fixture.
- Separate subgraph-branch and process-crash nested-loop placeholders remain outside this proof. UI binary not rebuilt in this step.

### 2026-09-28 — desktop durable history after cold restart

- Fixed active daemon list refresh dropping finished runs; UI now reads latest 100 registry rows read-only, folds durable terminal events over stale registry status, and restores event streams. No registry migration or status writes during inspection.
- Recorded history uses original event timestamps; persisted terminal events clear pending approvals.
- Verification: persistence inspection tests 2 passed; UI suite 67 passed, additional recorded-time/terminal regression passed; strict Clippy for UI/persistence all targets passed. Rebuilt and installed SurgePreview.
- Native cold restart: DAEMON LIVE, 23 runs/0 active (previously only 15); opened run 01M3MC9ZZ6AP0R1RVE2DH2AWR2 and observed 11:28:18 RunCompleted, verify_app passed, immutable report and application artifacts.
- Remaining: latest-100 global history has no pagination/project filtering; history reads precede connection and performance remains unmeasured. Result preview/open-worktree journey remains unfinished. Registry status lifecycle still needs repair at the producer; the UI event-derived status handles existing stale rows.

#### Follow-up diagnosis: registry terminal lifecycle

Source inspection found no orchestrator call to `Storage::set_run_status`; `run_task::execute` closes the writer after completion, while per-run view folding intentionally treats RunCompleted as a no-op for the registry. The field documentation claims terminal writes occur elsewhere, but current engine sources do not implement that transition. This explains completed event logs with bootstrapping/crashed registry rows. Fix must persist terminal summary after durable completion (and preserve parked semantics), with complete/fail/abort and restart tests. UI recovery is evidence-based mitigation, not a producer fix.

### 2026-09-28 — open actual result from Runs

- Added exact non-mutating registry lookup and native Open result folder action. Resolves persisted per-run directory asynchronously; missing/relative/file paths report errors. No shell interpolation or guessed worktree.
- UI tests 3 passed including real button click; persistence exact lookup tests 2 passed; both crates strict all-target Clippy and changed-file rustfmt passed. Parent built/deployed SurgePreview and independently reviewed lookup, validation, and action wiring.
- Native acceptance: selecting the completed timer on Fleet opened its Runs card; Open result folder opened Finder at ~/.surge/worktrees/run-01M3MC9ZZ6AP0R1RVE2DH2AWR2 with index.html/app.js/styles.css/tests/verification-report.toml visibly present.
- Built-in Button has Enter/Space handlers; native Tab did not expose focused control in AX, so full keyboard journey remains unproven. Feedback truncated; error detail usability remains to review.
- User emphasized GPUI/Zed capabilities: native diff/preview/Checks inside Surge remain required; folder opening is an auxiliary action. Current gpui-kit=0.7.0 exposes component::input::Editor with read-only mode; no exported DiffView/DiffEditor in installed gpui-component0.7. Zed's editor/git UI are distinct crates, so reuse must account for dependency boundary rather than assume GPUI alone includes them.

### 2026-09-28 — native Changes implementation in progress

- User explicitly requested GPUI-native review rather than relying on external folder opening. Current approach: read-only Editor component, per-file git2 patches, exact persisted worktree and validated bootstrap pinned base, async refresh and explicit output limits.
- Added `Storage::inspect_existing_run_capture`: one read-only SQLite transaction resolves exact run ownership and reuses full journal decoding/fingerprint checks. Missing registry returns None; malformed/future/duplicate records fail. New tests first failed for missing API, then passed; all 374 persistence lib tests passed, strict all-target Clippy passed.
- Independent live oracle: timer captured base e33278c855a92c914686392e914d90f50d5b5e11; README has +55/-2, 11 additional untracked files including tests/timer.test.js. Native inspection still pending.
- Fixed terminal stage chips: regression reproduced RunCompleted leaving phase Running; fold now completes terminal node and settles running chips on failure/abort. Regression green.

#### Changes native acceptance (initial build)

- Cold launch → Fleet completed timer → Runs Changes displayed captured base e33278c855a92c914686392e914d90f50d5b5e11 and all12 expected files. Native README selection showed old/new text; Refresh reread files. Switching to another run cleared old result.
- Review found Refresh reset selectedfile and unbornHEAD surfaced raw error; both sent for repair with tests. Native diff initially monochrome; found installed GPUI TextDecoration API and requested hunk-aware native colors with Unicode/header tests. Final deployment/acceptance pending those repairs.

#### Changes final native verification

- Rebuilt and deployed final SurgePreview. Native timer Changes shows all12files, exact pinned base, green additions/red deletions/accent hunk headers. SelectedREADME then Refresh: sameREADME remainedselected and samepatchvisible. Screenshot directly inspected at1280x800.
- Final parent-run UI gate:82unit+5integrationtests passed. Agent strictClippy/fmt passed. Persistence374libtests+strictClippy passed.
- Read-only GPUI Editor source and render/input regression verified; file contents are never written by Changes. UnbornHEAD regression now shows newfiles againstemptybase; invalid pinnedcommit errors remain errors.
- Remaining: full keyboard journey and large-repository latency not proven; output capped200files/256KiBperpatch/2MiBtotal with explicit partial notices. Legacyruns without capturedbase show labeledHEADworkingdiff and exclude committedrunhistory. Built-inPreview/Checks and broaderproductacceptance still outstanding.

### 2026-09-28 — native Checks in progress

- UI goal: Activity/Changes/Checks navigation and read-only latest immutable verification report with commands/results/notes, producer/session provenance, explicit saved-snapshot attribution and missing/corrupt/contradictory-state handling. Native acceptance pending.
- Added ArtifactStore::open_bounded(run,hash,max_bytes): reads at most limit+1, fails TooLarge, verifies expectedhash, never uses mutable source fallback. RedmissingAPI observed; new boundary/hash/emptyzero/missing tests pass; all375persistence libtests pass, strictalltargetsClippy passed before finaltest-only expansion.

#### Checks final native acceptance

- Built/deployed SurgePreview with Activity/Changes/Checks. Timer Checks opened real immutable report (18checks), readable command/result/note text. Scrolledtoend: exactrun01M3MC9ZZ6AP0R1RVE2DH2AWR2,nodeverify_app,event40,timestamp2026-09-28T16:28:13.488Z,hashc4e59f03e5152611c9b393c2d709a66753afce046803aaf3d57d93a286a25d62,producerverifier@2.0/runtimecodex-acp visiblypresent.
- Switchedtofailedlooprun: UI showed Checks unverified / No saved verification report, clearingprevioussuccess. ReturnedtosuccessfultimerandleftChecksopen.
- Parent finalUIgate87unit+5integrationpassed; agentstrictClippy/fmtpassed. Persistence375libtests passed. Read-onlyEditor andstalegeneration tests; corruption/invalidlatest/nofallback/unknownresults/contradiction/provenance covered.
- Scope: displays hash-checked saved report claims; doesnotreruncommands or prove currentworktreefreshness/independentexitreceipts. Evidence references aredisplayed,notautomaticallyopened. Eventhistoryscan unbounded; reportbytes capped1MiB. Clipboardcopy/fullkeyboardjourney remainunverified. NativePreview and executablecommandreceiptintegration remainoutstanding.
