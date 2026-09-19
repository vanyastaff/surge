<!-- Rust Code Studio acceptance ledger for .rust-studio/specs/autonomous-task-orchestration/spec.md.
     Authored 2026-09-18 alongside the running T1/T2 wave. One gate per acceptance criterion in
     spec.md §Acceptance criteria (18 criteria; the schema criterion is split into two gates so
     each CHECK stays single). Test filters are scoped to the test files / names the task
     breakdown (tasks.md §Expected files) assigns to each criterion; sharpen a filter to an
     exact test name when that test lands. -->

# Acceptance: Autonomous task orchestration v1

Spec: spec.md

- [x] G1: t1 dispatches first; t2 is not dispatched until t1's ledger row is verified
  CHECK: cargo nextest run -p surge-daemon -E 'binary(ato_outer_test)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: rs-acceptance/v1 def=21adfbbe4b1963a3 exit=0 expect=matched out=53f1feb07a2fff65:647 cwd=. shell=sh at=2026-09-19T02:29:10.515Z

- [x] G2: after t1 is verified, the project branch contains t1's commits and t2's worktree is based on it
  CHECK: cargo nextest run -p surge-daemon -E 'binary(ato_outer_test) and test(outer_scenario_pause_reprioritise_and_dependency_order)'
  EXPECT: /1 test run: 1 passed/
  EVIDENCE: rs-acceptance/v1 def=9a7955acde5ab68a exit=0 expect=matched out=45a99b1a9e363fc6:471 cwd=. shell=sh at=2026-09-19T02:34:05.629Z

- [x] G3: a merge conflict marks the row Failed{merge_conflict}, logs EscalationRequested, and keeps t2 blocked
  CHECK: cargo nextest run -p surge-daemon -E 'binary(daemon_task_scheduler) and test(merge_conflict)'
  EXPECT: /1 test run: 1 passed/
  EVIDENCE: rs-acceptance/v1 def=adfaf08cf72ee199 exit=0 expect=matched out=b3c88addad9c0ae7:457 cwd=. shell=sh at=2026-09-19T02:29:11.816Z

- [x] G4: a dependency in Failed is listed by `surge ready` as blocked_by_failed and is not dispatched; `surge task requeue` unblocks it
  CHECK: cargo nextest run -p surge-cli -E 'test(blocked_by_failed)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: rs-acceptance/v1 def=8c797a3d7908c96f exit=0 expect=matched out=0def4c4b1a7751b6:772 cwd=. shell=sh at=2026-09-19T01:39:04.611Z

- [x] G5: a Low task skipped aging_threshold times rises one effective priority level; order is total, deterministic, deps dominate (property tests on QueuePolicy)
  CHECK: cargo nextest run -p surge-orchestrator -E 'test(scheduler::policy)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: rs-acceptance/v1 def=3dd17e80f73aef0b exit=0 expect=matched out=9411a5f60a8f780e:2386 cwd=. shell=sh at=2026-09-18T23:57:31.942Z

- [ ] G6: a classifier answer `use: bug-fix@1` makes the run's PipelineMaterialized graph equal the template with task bindings filled, with no generator turn
  CHECK: cargo nextest run -p surge-daemon -E 'binary(ato_outer_test) and test(classif)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: failed at=2026-09-19T03:03:50.016Z exit=4 expect=unmatched

- [ ] G7: a `compose` answer is validated with FlowPurpose::Task (unverified success path and same-runtime verifier rejected and retried) and on success the file exists in `.surge/flows/` with ComposedArtifactInstalled{kind: Flow} after the gate
  CHECK: cargo nextest run -p surge-orchestrator -E 'binary(task_run_select_test)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: failed at=2026-09-19T03:03:50.341Z exit=94 expect=unmatched

- [ ] G8: a composed profile with `authority = true` or a bundled name is rejected at post-processing with a named error
  CHECK: cargo nextest run -E 'test(composed_profile)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: failed at=2026-09-19T02:34:12.311Z exit=4 expect=unmatched

- [x] G9: a `.surge/profiles/x-1.0.toml` resolves with Provenance::Project over home and bundled, and two repos never see each other's profiles
  CHECK: cargo nextest run -p surge-orchestrator -E 'binary(engine_project_layer_scoping) + binary(profile_registry_e2e)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: rs-acceptance/v1 def=3a40684cdd376700 exit=0 expect=matched out=afed59a3be6a6d4c:2714 cwd=. shell=sh at=2026-09-18T23:33:19.186Z

- [ ] G10: a GitHub issue candidate starts a planning run (not a work run); after approval `.surge/roadmap.toml` contains the task at the planner's insertion point and a queue row exists
  CHECK: cargo nextest run -p surge-daemon -E 'binary(daemon_intake_planning_run)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: failed at=2026-09-19T02:34:12.582Z exit=94 expect=unmatched

- [ ] G11: a patch conflicting with the running milestone is deferred to the next milestone without operator input
  CHECK: cargo nextest run -E 'test(defer_to_next_milestone)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: failed at=2026-09-19T02:34:13.315Z exit=4 expect=unmatched

- [x] G12: `surge task pause` stops new dispatch and halts the running run at its next stage boundary; resume continues
  CHECK: cargo nextest run -p surge-daemon -E 'binary(ato_outer_test) and test(pause)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: rs-acceptance/v1 def=636331334bcbe1e4 exit=0 expect=matched out=222b9128a4cc96cd:471 cwd=. shell=sh at=2026-09-19T02:33:25.177Z

- [x] G13: `surge task priority t3 critical` while t1 runs changes file and row so t3 dispatches next, and the running run's RunOrigin.roadmap_hash still shows the old hash
  CHECK: cargo nextest run -p surge-daemon -E 'binary(ato_outer_test) and test(priorit)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: rs-acceptance/v1 def=17800866dba4cd43 exit=0 expect=matched out=3a6f28314c4ca5f1:471 cwd=. shell=sh at=2026-09-19T02:33:25.919Z

- [ ] G14: editing `.surge/flows/bug-fix-1.0.toml` mid-run leaves the current task unaffected and the next task using it gets the edited content after the trust prompt
  CHECK: cargo nextest run -p surge-daemon -E 'binary(ato_outer_test) and test(edit)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: failed at=2026-09-19T02:34:13.769Z exit=4 expect=unmatched

- [x] G15: a fresh clone with an unpinned `.surge/` profile does not start the run, logs EscalationRequested{UntrustedProjectFile}, and `surge trust accept` pins it
  CHECK: cargo nextest run -E 'test(untrusted_project_file)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: rs-acceptance/v1 def=64d8c4d05e9d0de5 exit=0 expect=matched out=d647cba9ae3bbe2a:908 cwd=. shell=sh at=2026-09-19T02:33:41.254Z

- [x] G16: event schema migration 8→9 round-trips every supported version
  CHECK: cargo nextest run -p surge-core -E 'binary(migrations_v1_roundtrip)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: rs-acceptance/v1 def=10727e9753e17049 exit=0 expect=matched out=086a00d2ffb15ba0:873 cwd=. shell=sh at=2026-09-18T23:33:55.254Z

- [x] G17: adding a payload variant without a schema bump fails the pinned variant-set test
  CHECK: cargo nextest run -p surge-core -E 'test(event_payload_variant_set_is_pinned_to_schema_version)'
  EXPECT: /1 test run: 1 passed/
  EVIDENCE: rs-acceptance/v1 def=bd5190d0d25fe8ba exit=0 expect=matched out=e2092f88e48df02f:462 cwd=. shell=sh at=2026-09-18T23:33:56.012Z

- [x] G18: daemon restart with a row in Dispatched reconciles against the run — live run left alone, dead run marked Failed and the task re-queued once (attempt+1)
  CHECK: cargo nextest run -p surge-daemon -E 'binary(daemon_task_scheduler) and (test(claimed_but_unstarted_task_requeues_exactly_once) + test(live_run_is_left_dispatched))'
  EXPECT: /2 tests? run: 2 passed/
  EVIDENCE: rs-acceptance/v1 def=8d33575705def634 exit=0 expect=matched out=2ff6a6956e848632:570 cwd=. shell=sh at=2026-09-19T02:34:14.499Z

- [x] G19: `surge ready` no longer reports the worktree as the project path
  CHECK: cargo nextest run -p surge-cli -E 'test(ready) and test(project)'
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: rs-acceptance/v1 def=60e626790f3091c3 exit=0 expect=matched out=1f1a99d31f42d8c6:460 cwd=. shell=sh at=2026-09-19T01:44:00.012Z
