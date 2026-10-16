# Gate graph authority and historical projection closure

Status: **Stage 5b bounded test-shape repair in progress**. Local Stage5a ACCEPTABLE; domain/security Stage5b COMPLETE; generic Stage5b REDO-TO-BAR. No overall journey/native/Windows completion claimed. Repair dispatches: **1/3**.

Task identity: Stage A, a new graph-authority defect diagnosed in the actual Sol61 native planning journey. This does not reset Home/harness repair budgets (3/3 exhausted); PR150 fourth Windows repair remains unanswered and unapplied.

Source/destination: `/Users/vanyastafford/.codex/worktrees/results-planning-history/surge`, branch `codex/results-planning-history`, base `c3a3ab3a1b3b46bd0c3abd6ece1ca46cf85c7089`. Root verified Stage A production paths unchanged from the reviewed plan's a2ba09c basis. Results was independently reviewed and locally committed; no source owner overlap remains.

Pre-code evidence: `/tmp/surge-gate-revisit-diagnosis.md` and `/tmp/surge-gate-revisit-probe.log`, plus preserved native fixture `/private/tmp/surge-sol61-native-9tkig5g4`. The probe is a reduced actual-Rust witness, not full regression/native success. Revised full-closure plan below received ACCEPTABLE owner, critic and security reviews, including the mandatory old queued SQL barrier and exact second-author cold cut.

Authorization: active broad project journey goal permits this causal implementation and local stage commits. No push, new PR, merge, provider replay, retry of original fixture, memory write, Home/harness fourth repair or verification-receipt/sandbox work is inferred. Original native/runtime fixtures and `.l3home/` are outside the write zone. Tests use isolated retained homes and explicitly settle actual owners.

Workflow: build RED core/security, historical DB/index, real Engine edit/revisit and exact second-author daemon cold tests BEFORE their production fixes; preserve separate receipts. Implement the entire reviewed dependency closure, refresh focused/full affected gates, then independent Stage 5a before Stage 5b. Any repair finding/count is recorded here before dispatch, max three shared rounds. Do not certify completion from core-only or new-writes-only success. Shared Cargo target is leased exclusively to the builder until explicit release.

Reviewed plan SHA256: `29332536d13bf458da2ac84200746a13ef9b5270553dd0cc79948cb25fb98d8b`.

## Reviewed full plan

# Gate and verification graph authority — revised Stage A plan

Read-only design against Surge main a2ba09c, 2026-10-15. No repository edits, builds, providers, or original evidence-fixture opens. This scratch plan extends the initial core-only proposal to its complete normalized and historical authority closure. Stage B observability is separate. Existing Home/harness repair budgets are unchanged.

## Owner contract

The first PipelineMaterialized establishes this run's active graph. Each explicit GraphRevisionAccepted replaces it. Later PipelineMaterialized graphs are generated future-child artifacts, never parent authority. Preserve latest-generated-child handoff (bootstrap/handoff.rs444), executable graph_in_force (execution_state.rs367), TaskSplit routing/splices, legitimate verification invalidation, exact gate occurrence/decision/route identities, and all contradiction checks. No decision clearing to conceal the defect.

Normalize active graphs with the same existing with_default_escalation_gates function in memory AND SQLite. Normalized escalation gates are authority only by that canonical existing rule, not arbitrary child graph membership. VerificationContext::observe invalidations remain unchanged; ignoring a generated graph for authority does not license disabling any existing invalidation.

## Required production closure

1. surge-core/src/run_state.rs::RunMemory::apply_event1754: separate first-materialization initialization from unconditional accepted-revision replacement. An accepted revision remains authoritative across later generated materializations. Full RunState fold, live memory and cold load_memory agree. Preserve legitimate explicit revisions, including revocation.
2. surge-persistence/src/runs/verification.rs::maintain101: first materialization writes normalized graph only when graph_json is NULL; every accepted revision writes normalized graph. Claim classification continues using exact subject/criteria/report/artifact binding. Unauthorized claims must not alter the ledger or insert a proof. Do not infer authorization merely from a raw TaskVerified event.
3. Historical views require explicit correctness versioning, not just fixed future writes. Add the next per-run migration creating a singleton projection-semantics version initialized to untrusted 0 (with a constrained schema). Migration application is not proof that replay completed. Owned writer startup in storage.rs::upgrade_existing_run432, after both existing writer guards and schema compatibility validation and before publishing pools/writer tasks, transactionally rebuilds recomputable views with corrected semantics, then advances version to current in that SAME successful transaction. Fresh runs must also receive current version through the correct initialization path before they accept appends or expose trusted projections. Failure leaves version untrusted and refuses writer publication; reopening retries safely. Unknown future versions fail closed.
4. Reuse views.rs::rebuild735, which preserves durable graph_snapshots. Factor the existing writer.rs460 reset/replay transaction body into one private owner helper shared by explicit rebuild and startup upgrade. Replay rows in exact seq order using existing version/schema-aware migration/decoding semantics, not the current helper's unchecked current-VersionedEventPayload-only decode. Bound row buffering; do not collect an unbounded journal solely for upgrade. Preserve append-only events, graph checkpoints/cursors, accepted gate/stage commits, artifacts, human-acceptance flags, and all current view semantics through replay. Never replay ACP/provider commands. Rebuild failure is a storage/open error, not permission to keep stale trusted data. Correctness marker must only advance after every replayed row succeeds.
5. Readonly historical consumers must fail closed BEFORE any writer has opened the old run. inspect_verification_proofs (verification.rs172) checks the current correctness marker inside its existing readonly transaction before exposing verified=true; missing/0/future version returns an explicit unconfirmed/unsupported storage error, never trusts normalized bool alone and never migrates. The normalized task_ledger reader (reader.rs290) needs the same trusted-projection check so sync_task_ledger_index cannot republish stale positives. Prefer explicit refusal over silently representing a stale completed/verified projection as freshly recomputed. Raw journal inspection stays available.
6. Cross-run task_ledger_index requires a write barrier against already-open OLD daemon pools, not just a one-time verified downgrade. Trace found one production write SQL: TaskLedgerStore::upsert (task_ledger.rs165), an explicit-column INSERT ... ON CONFLICT DO UPDATE; list/get are the readers at214/262. Other repository SQL writes are historical migrations that only downgrade verified to0 and a test fixture. sync_task_ledger_index(storage.rs148) is the normal publisher and must retain the per-run current-semantic reader guard above.

   The next registry migration transactionally REBUILDS task_ledger_index, retaining its full current columns, PK and four indexes, adding projection_semantics INTEGER NOT NULL CHECK(projection_semantics=1) with NO DEFAULT. Copy every old row with verified=0 and projection_semantics=1; preserve all status, project, timestamps, provenance and human-history fields exactly. New upsert explicitly supplies1 and updates the semantics field explicitly. Do NOT use ALTER ADD COLUMN with a default, a nominal epoch old UPSERT can preserve, or an INSERT trigger exempting conflict updates. The mandatory omitted column causes OLD INSERT/UPSERT to fail NOT NULL BEFORE conflict handling, both for an absent key and a key already holding a newly trusted current row. Old prepared statements auto-reprepare on schema change and encounter the same constraint. Therefore an old stale in-memory verified1 snapshot cannot cross the upgrade boundary even after a new daemon has rebuilt that run and published a valid current row. Already-open old pools need not be assumed quiescent; no cross-process shutdown claim.

   New list/get require supported semantics and never expose an unsupported row as verified. Current migration copy is conservatively unverified, not an assertion of recomputed historical proof. After correct owned per-run rebuild, normal sync may restore actual valid positives. Atomic registry schema rebuild means old readers can observe their old snapshot or the committed downgraded table, but cannot publish stale positives into the new table. This guarantee governs CURRENT consumers of the new registry; it does not retroactively change what an old binary already displayed in an old SQLite read snapshot. In-flight old writes already holding a write transaction finish before migration obtains exclusivity, and their output is included in the conservative copy. Old writes executing afterward fail.

   Source-compatibility impact: old code attempting index upsert after upgrade receives its normal SQL error; do not add a compatibility fallback that omits the barrier. New registry schema still participates in the existing unknown-migration refusal on fresh old opens. No destructive row/history deletion, no authorization grant. Preserve every column of the actual current source schema; use its exact CREATE TABLE and index SQL as the oracle.

Marker schema and error naming remain private persistence implementation details. No new scheduler, execution owner, verification receipt contract, provider behavior or authority grant. Additive schema migration intentionally makes older binaries refuse unknown migrations; test that supported upgrade and future-schema refusal remain intact.

## RED-first oracles

Core + replay + SQLite use fixed literal graphs and well-bound report/artifact fixtures; expected permissions are literal, not derived from the implementation under test:

- Parent A does NOT authorize verification node N; generated C DOES. After A,C a well-bound N claim is Unauthorized in incremental memory, full fold, cold memory, append-maintained SQLite and rebuilt SQLite. No ledger/proof positive and no index positive.
- A authorizes N; accepted revision B revokes N; generated C authorizes N. C must not restore authority. Test explicit later accepted revision restoring N works, with fresh subject/criteria/evidence as required by existing invalidation contracts.
- Accepted B survives C; canonical escalation normalization matches across core and normalized graph_json. Preserve the existing legitimate verification/unauthorized/invalidation and explicit revision suites.
- A,generated C, valid edit answer, StageCompleted, revised generated C, fresh gate revisit: literal HumanGate purpose, retired first decision, fresh distinct request/entry, no false conflicting decision. Stale and wrong request answers are refused. Keep genuine duplicate/conflicting answers refused.
- Seed OLD database using pre-new-migration schema and actual old maintained graph/proof/index state. Before writer upgrade readonly proof AND ledger/index access cannot produce trusted stale verification; files/paths unchanged. Upgrade removes false proofs and corrects authority, preserves valid proofs/human flags/checkpoints/commit history; idempotent reopen and explicit rebuild agree. Include old falsely denied legitimate A claim, revoked-B false positive, absent sealed bytes, corrupt event/unsupported schema and injected mid-replay failure. Failed replay cannot advance marker or expose partial rebuilt views. Exact event bytes/count and graph snapshot bytes/cursor remain unchanged.
- Fresh current DB, old no-verification DB, current DB and future marker/schema each exercise initialization/refusal paths. Registry migration downgrades stale positives; attempted sync from untrusted run cannot restore them; sync after corrected replay may restore only valid evidence. Add a real SQLite TWO-CONNECTION regression: keep a pre-upgrade connection alive, obtain and hold an old verified1 source snapshot, warm/prepare the EXACT old upsert SQL; migrate on the other connection; execute old queued SQL against both absent key and newly-current existing key AFTER a new valid positive has been published. Both old writes must fail the mandatory-column constraint, existing row bytes must remain unchanged, and list/get must retain only current data. Repeat with an old transaction holding the writer before migration (it completes first, then its positive is downgraded) and with a new current upsert succeeding. Assert all migrated human/history fields and indexes survive. Do not simulate the legacy writer with new SQL or rely solely on an epoch predicate in readers.

## Real Engine and exact cold cut

Extend bootstrap_validation_retry_test.rs using existing BootstrapHarness and real valid Flow artifacts: description approve, roadmap approve, valid flow generation, flow gate edit, second valid flow generation, fresh second flow gate approve/handoff. Assert two exact distinct flow-gate requests and monotonically increasing owning entries, one edit effect, literal route outcomes, no RecoveryRequired/fabricated terminal. Join driver and settle bridges/writers on success AND assertion failure.

Existing debug SURGE_ROUTE_COMMIT_EXIT selects only the first occurrence (run_task.rs2498-2522). Extend its private debug-only selector with an explicit positive durable occurrence number, retaining old first-occurrence behavior. Count matching StageEntered occurrences from the recorded committed prefix; require selected entry is the current entry AND its route is already committed. Invalid/zero selectors do not exit. Unit-test selector parsing and exact first/second occurrence discrimination. Release builds retain the existing no-selector/no-exit behavior; no production policy/env surface.

Actual daemon subprocess fixture cuts AFTER SECOND flow_generator's durable route, before revisited gate execution. The parent verifies the literal journal prefix contains two author completions/routes, the first answered/edit-consumed gate, and no second answer; then reopens fresh Storage+Engine/host. Assert author/provider invocation count stays exactly two, no replay of second author, fresh gate identity, old/wrong answer refusal, correct new approval and exactly one handoff child with latest revised Flow. Compare live pre-cut, cold memory and full fold authority against literal parent membership. Use existing authenticated provider/MCP/child-owner fixture, deterministic exit/barriers and actual settlement; no sleep-only cut, fabricated journal, or fake engine-result oracle. Test Windows fixture cleanup/owner release and compile debug/release seam coverage.

## Scope/files and gates

Expected owners: core/run_state.rs and nearby tests; persistence runs/verification.rs, views.rs, writer.rs, storage.rs, reader.rs, migrations.rs plus next per-run/registry SQL and owner-local tests; orchestrator engine/execution_state tests, bootstrap_validation_retry_test.rs and narrowly debug-only run_task selector; daemon existing cold bootstrap/gate fixture and test sibling if needed. Do not overlap Results builder's inspection.rs/UI worktree. Coordinate shared Cargo ownership; this plan writes only /tmp.

Capture real failing core/security, old-database, outer Engine and exact second-author cold tests before production changes. Run focused core graph/gate/verification, persistence migration/rebuild/proof/index, engine edit/revision/verification and daemon cold-gate/TaskSplit suites; strict default+all-features affected/all-target Clippy, rustfmt/diff check, workspace gates and fresh Windows CI. Root owns actual fresh isolated Sol61 native journey and final revision-bound platform evidence. Preserve original failure fixture untouched. Stage B durable diagnostic/display closure remains separately planned; completing Stage A does not complete the user's full journey.

## Maintainer pre-code verdict

ACCEPTABLE for independent review with this full closure. Core-only and new-writes-only fixes are RESHAPE NEEDED. Historical safety is explicit: untrusted old projection refuses readonly authority, exclusive owned upgrade rebuilds transactionally, registry positives are conservatively downgraded, current semantics are certified only after replay, and an obligatory no-default index column rejects queued OLD SQL before conflict updates. No historical event or decision is deleted or rewritten. Costs are one owned replay per old run and conservative temporarily unavailable verification until upgrade; readonly operations never perform repair. Implementation must report any new consumer of normalized verified bits found during source tracing before narrowing this closure.


## Historical concurrency amendment: inspected evidence and scratch SQL proof

Security correctly rejected a one-time registry downgrade: old TaskLedgerStore::upsert can publish an already-read stale bool after it. The chosen schema barrier closes this exact existing write path; a per-run marker alone cannot, since the run may already be upgraded by the time old SQL executes.

A scratch Python sqlite3 two-connection experiment (temporary DB only; no repository/original-fixture access) executed the existing INSERT explicit-column/ON CONFLICT shape before and after an exclusive table rebuild with a required no-default projection_semantics column. The same old connection and statement were reused. Existing conflict key and new key both produced `IntegrityError NOT NULL constraint failed: idx.projection_semantics`; the current existing row stayed `('r','t',1,1)`; explicit new semantics-aware UPSERT succeeded. This is a reduced SQLite mechanism witness, not pinned rusqlite/native Windows acceptance. Required production regression uses the exact pinned rusqlite SQL/schema and old-connection lifetime.

Decisive SQL invariant:
`projection_semantics INTEGER NOT NULL CHECK(projection_semantics=1)`
with NO DEFAULT; old explicit-column INSERT does not provide it. All supported new writers provide it; no supported direct UPDATE writer restores positives. Search performed: `rg -n 'INSERT.*task_ledger_index|UPDATE task_ledger_index|DELETE FROM task_ledger_index|FROM task_ledger_index' crates scripts`. Future direct positive writers must join this owner contract.

Revised pre-code verdict: ACCEPTABLE for independent security/critic review with this mandatory schema write barrier and exact old-queued-SQL regression included. A defaulted/nullable column, field ignored by old ON CONFLICT, only a marker, or quiescence assumed from fresh-open migration checks remains RESHAPE NEEDED.

## Build coordination

- `gate_authority_builder`: sole Cargo lease; core/persistence production and tests.
- `gate_cold_test_builder`: source only, orchestrator outer Engine regression and narrow debug cut selector/tests.
- `gate_daemon_cold_builder`: source only, actual daemon cold fixture/tests.
- Authority production changes held until both outer RED receipts; selector instrumentation may precede them.
- No Cargo subprocess currently delegated.

Bounded fixture amendment: daemon owner may add `tests/bootstrap_gate_cold_recovery.rs`, `tests/fixtures/bootstrap_gate_cold.rs`, and optional allowlisted `bootstrap_artifacts` in test-only `surge-acp/src/bin/mock_acp_agent/stage_mcp.rs` (safe conventional names, validated keys, existing split fixture unchanged). No provider/runtime production behavior change.

Existing upgrade fixture correction: the old test manufactured a `stage_executions` row without journal events. It now seeds literal StageEntered, SessionOpened, TokensConsumed and the original RunAborted, retaining stage identity/cost/token migration assertions. Journal-backed costSome(1.25) requires known_cost_usd=Some(1.25), rather than the prior fabricated None.

Test infrastructure ripple: `surge-acp` mock artifact fixture joins default/all-feature all-target Clippy and relevant fixture tests. Core RED exit101 (one failed); registry RED exit101 (one failed); actual Engine outer RED exit100 (one failed, contradictory durable occurrences at second flow_gate); persistence negative RED exit101 (six failed); canonical SQLite escalation RED exit101 (one failed). Logs `/tmp/surge-gate-authority-{core,registry,engine,persistence,negative,normalization}-red.log`.

## Initial implementation evidence (independent reviews still pending)

All commands run from this worktree with `login:false`, exclusive Cargo lease, and:
`CARGO_TARGET_DIR=/Users/vanyastafford/Develop/surge/target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.

| Invocation | Receipt | Exit/result |
|---|---|---|
| `cargo test -p surge-core generated_child_cannot -- --nocapture` | `/tmp/surge-gate-authority-core-red.log` | 101; 1 behavioral failure |
| `cargo test -p surge-core accepted_revision_restores_authority -- --nocapture` | `/tmp/surge-gate-authority-restore-red.log` | 101; incremental restore failure; full fold was already correct |
| `cargo test -p surge-persistence graph_authority_ -- --nocapture` | `/tmp/surge-gate-authority-additional-red.log` | 101; 8 behavioral failures |
| `cargo test -p surge-persistence graph_authority_sql_replay_failure -- --nocapture` | `/tmp/surge-gate-authority-sql-replay-red.log` | 101; injected SQL replay refusal missing |
| `cargo nextest run --locked -p surge-orchestrator --test bootstrap_validation_retry_test -E 'test(valid_flow_edit_requires_fresh_second_gate)'` | `/tmp/surge-gate-authority-engine-red.log` | 100; actual second gate contradictory durable occurrence |
| `cargo build --locked -p surge-acp --bin mock_acp_agent` | `/tmp/surge-gate-authority-mock-build.log` | 0 |
| `SURGE_TEST_MOCK_ACP_BINARY=/Users/vanyastafford/Develop/surge/target/debug/mock_acp_agent cargo test --locked -p surge-daemon --test bootstrap_gate_cold_recovery bootstrap_gate_cold::second_flow_author_route_recovers_fresh_gate_and_latest_child_once -- --exact --nocapture` | `/tmp/surge-gate-authority-daemon-red3.log` | 101; authentic second author route cut; fresh second gate refused at seq95/96 |
| same daemon invocation, earlier setup attempts | `/tmp/surge-gate-authority-daemon-red.log`, `/tmp/surge-gate-authority-daemon-setup2.log` | 101; setup only: canonical worktree path rejected `/var` symlink ancestry; not counted as behavioral RED |
| `cargo nextest run --locked -p surge-core -p surge-persistence -E 'test(graph_authority) \| test(generated_child_cannot) \| test(accepted_revision_restores) \| test(existing_run_upgrade)'` | `/tmp/surge-gate-authority-focused-green2.log` | 0; 14/14 passed |
| same focused invocation, first compile | `/tmp/surge-gate-authority-focused-green.log` | 101; helper concrete pooled argument prevented transaction coercion; corrected private reader helper boundary |
| actual Engine outer invocation above | `/tmp/surge-gate-authority-engine-green.log` | 0; 1/1 passed |
| current mock build invocation above | `/tmp/surge-gate-authority-mock-build-current.log` | 0 |
| actual daemon exact-cut invocation above | `/tmp/surge-gate-authority-daemon-green.log` | 0; 1/1 passed; subsequent fixture cleanup strengthening still requires refresh |
| `cargo nextest run --locked -p surge-core -p surge-persistence --no-fail-fast` | `/tmp/surge-gate-authority-core-persistence-full.log` | 0; 1596 passed, 1 skipped |
| `cargo clippy --locked -p surge-core -p surge-persistence --all-targets --all-features -- -D warnings` | `/tmp/surge-gate-authority-core-persistence-clippy2.log` | 0 |
| same strict invocation, first pass | `/tmp/surge-gate-authority-core-persistence-clippy.log` | 101; new test style findings corrected, no suppressions |

Additional direct replay tests added after grouped historical RED, without claiming a separate earlier baseline RED: old-version decoding/raw-byte preservation, unsupported row event schema despite current-decodable bytes, and unavailable-cost replay. All passed in the 1596-test gate.

New implementation boundary: private `runs/projection.rs` owns marker validation and exclusive bounded/version-aware reset/replay; `reader_views.rs::task_ledger` takes `&Connection` so the existing query can consume the same transaction snapshot as its marker guard. No new normalized authority consumer. Current registry SQL is a private owner constant reused by the two-connection regression to publish a fresh positive before executing the exact frozen old SQL.

Build remains in progress: daemon failure cleanup evidence strengthening, remaining full affected/workspace/default/all-feature/release seam gates, source hash/freeze, and root-owned independent Stage 5a then Stage 5b are open. Windows execution and actual fresh Sol61 native acceptance are root-owned and not claimed here. Cargo lease remains held by builder.

Resource boundary: `cargo nextest run --locked -p surge-orchestrator -p surge-acp --no-fail-fast` compiled successfully, but root requested resource interruption at low disk. Graceful SIGINT landed just after test scheduling began: `/tmp/surge-gate-authority-orchestrator-acp-full.log`, exit100, 447 passed, 7 SIGINT, 1197 unrun (454/1651 attempted,9 skipped). This is not a completed gate or a behavioral regression. No Cargo/rustc/new mock processes remained. Root owns target artifact cleanup; builder released artifact mutation lease and froze further build scheduling. No runtime/user data deleted. A pre-existing unrelated PPID1 `work_item_route_test-b388ca25fea0ae97` PID52463 was reported to root and left untouched.

Build alias: parent authorized ephemeral `target` layout for fixed legacy mock fixture paths. Verified absent path and existing shared target before creation. Replaced temporary root symlink only after terminal Cargo with actual gitignored `target/` directory and `target/debug -> /Users/vanyastafford/Develop/surge/target/debug`; `git check-ignore target/debug` confirms ignored. No source or configuration changes.

Additional historical legacy matrix now includes explicit accepted B revoking N, followed by generated C restoring old buggy projection; current replay must reject it. This closes the reviewed explicit revocation case alongside initial unauthorized and falsely denied legitimate parent cases; refresh remains pending.

## Superseded source freeze and resource-limited gate profile

Rust/SQL source frozen after the approved additional daemon variant and final formatting. Deterministic manifest: `/tmp/surge-gate-authority-source-manifest.sha256`, **956 files**, aggregate SHA256 **318c047dccbb138294aa972ae30154a602dd46ae274a23369dc8183a42be8f87** (sorted `rg --files -g '*.rs' -g '*.sql' crates scripts`; each manifest line is SHA256 + relative path). No production changes were needed for the authentic projection-rebuild variant. Independent reviews remain pending.

Final historical focused receipt `/tmp/surge-gate-authority-historical-final-focused.log`: `cargo nextest run --locked -p surge-persistence --lib -E 'test(graph_authority) | test(runs::projection::tests)'`, **exit0,12/12 passed**. This includes explicit historical B revocation and exact index schema/PK/four index definitions. A final current-registry-reader unsupported epoch test is included in frozen source and awaits workspace execution.

Daemon cleanup evidence:
- `/tmp/surge-gate-authority-unwind-red.log`: exact intentional assertion panic caught; prior force-kill cleanup lacked durable RunAborted. Exit101; recorded kill/join signal9 and authenticated groups Empty, without claiming graceful settlement.
- `/tmp/surge-gate-authority-cold-unwind-green.log`: approved bounded graceful stop/actual joins, exit0,4/4 passed.
- `/tmp/surge-gate-authority-cold-final-focused.log`: additional marker0 variant initially asserted after transport readiness but before asynchronous run opening; exit101,4/5 passed. Preserved separately as an oracle ordering failure, not a production regression; evidence home retained.
- `/tmp/surge-gate-authority-cold-final-focused2.log`: check moved after genuine fresh durable gate request and before answering; **exit0,5/5 passed**. Original no-damage cold, actual unwind settlement and new marker0 owned replay all pass. Variant compares actual4 stage/3 gate commit rows (including routed_seq), raw prefix and snapshot bytes before review and after handoff. This is current-schema untrusted-marker coverage, not an old-schema fixture or invented prior RED.
- `/tmp/surge-gate-authority-selector-debug.log`: `cargo nextest run --locked -p surge-orchestrator --lib -E 'test(route_commit_selector)'`, exit0,1/1 passed before resource profile override.
- Core/persistence strict default and all-feature all-target Clippy passed; receipts `/tmp/surge-gate-authority-core-persistence-clippy-default.log` and `/tmp/surge-gate-authority-core-persistence-clippy2.log`.

Workspace build-only preparation `/tmp/surge-gate-authority-workspace-build-list.log` failed **exit101 ENOSPC** (linker/rust-objcopy); no fixtures ran. This is a resource setup failure, not a behavioral gate result. Builder froze Cargo and released artifact mutation lease. Root audited removal of inactive rebuildable Cargo outputs only; receipts `/tmp/surge-stageA-native-cache-cleanup.json` and `/tmp/surge-stageA-test-executable-cleanup.json`. The latter removed252 inactive hashed executables, preserving libraries, standalone helpers, source, runtime/logs and the unrelated active orphan image; free space recovered to10GiB.

Root-approved final local test environment adds **`CARGO_PROFILE_TEST_STRIP=symbols CARGO_BUILD_JOBS=2`** to the original debug0 env. Installed rustc confirms stripping and debug assertions are independent; source scan found no symbol-table/backtrace-name assertion or dynamic-symbol lookup dependency in workspace tests. Symbols may be unavailable in backtraces: this override is recorded as local resource-limited evidence, **not literal CI-profile or Windows evidence**. No Cargo.toml/CI/lint settings or suppressions were changed. Debug selector and actual code99 cold seam will be re-exercised under this profile. Workspace stripped build/list is pending in `/tmp/surge-gate-authority-workspace-stripped-build-list.log`; actual full run follows a disk checkpoint. Release selector and strict broader gates remain open.


## Historical journal completeness audit (initial build, repair count 0/3)

Root's final completeness audit found that owner replay accepted any positive ordered sequence, although `RunState::fold` requires a contiguous prefix. Normal writer transactions allocate omitted INTEGER PRIMARY KEY sequences and roll event/view failures back together, but explicit historical inserts can leave seq1,3 with a missing accepted GraphRevisionAccepted revocation. The reader proof guard would trust the incorrectly published marker1. This is inside the historical authority trust closure; no general tamper framework or current-reader rescan was added.

Pinned real historical RED: `/tmp/surge-gate-authority-gap-red.log`, `cargo test --locked -p surge-persistence --lib graph_authority_gap -- --nocapture`, exit101, 0 passed/1 failed. Literal diagnostic: `error=None, exposed_verified=true`. Fixture seeds the missing revocation gap through explicit INSERTs from the outset, without disabling append-only triggers or deleting events.

`projection::rebuild_owned` now requires expected_seq=1 and each consecutive sequence before payload migration or view maintenance. A gap returns an error inside the exclusive transaction before writer publication; marker/views rollback. Focused initial GREEN `/tmp/surge-gate-authority-gap-focused-green.log`, `cargo test --locked -p surge-persistence --lib graph_authority_ -- --nocapture`, exit0,11/11 passed. Includes failed append batch rollback and reuse of seq2, proving normal failure does not cause a durable gap. Final strengthened oracle additionally compares every context/criteria/proof column and complete graph_snapshot tuples, raw event columns, ledger human flags and repeated ownership refusal.

The previous 956-file manifest is retained as superseded evidence. Stripped workspace list `/tmp/surge-gate-authority-workspace-stripped-build-list.log` completed exit0: build/list evidence only, no full test run. Current-source focused closure and a new source freeze precede final workspace execution. Cargo lease remains held by builder; independent5a/5b, Windows execution and actual fresh native acceptance remain open/root-owned. PR150 Windows patch remains excluded.


Current gap-complete Rust/SQL freeze: `/tmp/surge-gate-authority-source-manifest-gap.sha256`,956 files, aggregate SHA256 **997b1a866a9fdaa5c9ff3e3f6dfd895d4d0dcb87fd7897ddfa5db4aa6644fe26**. Root independently verified the manifest hash. Strengthened gap oracle compares context/criteria/proof rows and full snapshot tuple. No source changes pending.

Attempted focused closure command (same base env plus jobs2/strip): `cargo nextest run --locked -p surge-core -p surge-persistence -p surge-orchestrator -p surge-daemon -p surge-acp -E 'test(graph_authority) or test(route_commit_selector_requires_exact_durable_occurrence_and_route) or test(valid_flow_edit_requires_fresh_second_gate) or binary(bootstrap_gate_cold_recovery) or test(bootstrap_fixture_artifacts_are_allowlisted_before_any_write)' --no-fail-fast`, receipt `/tmp/surge-gate-authority-gap-closure-focused.log`. Session89699 exited130 after authorized graceful SIGINT at1.6GiB free; compile only,0 tests executed. No behavior/gate claim. Cargo/rustc/linker process inventory empty afterwards. Artifact mutation lease returned to root; scheduling frozen pending audited rebuildable-output cleanup. Subsequent focused/full execution will use one workspace feature union, avoiding repeated subset output duplication.


Root cleanup receipt `/tmp/surge-stageA-subset-executable-cleanup.json`:108 inactive hashed Mach-O outputs from interrupted subset removed,3783MiB freed; source/libraries/helpers/runtime/orphan retained. Artifact lease returned; resumed one workspace feature union. Final focused command uses the common final env plus `SURGE_TEST_MOCK_ACP_BINARY=/Users/vanyastafford/Develop/surge/target/debug/mock_acp_agent`: `cargo nextest run --locked --workspace -E 'test(graph_authority) or test(generated_child_cannot) or test(accepted_revision_restores) or test(runs::projection::tests) or test(route_commit_selector_requires_exact_durable_occurrence_and_route) or test(valid_flow_edit_requires_fresh_second_gate) or binary(bootstrap_gate_cold_recovery) or test(bootstrap_fixture_artifacts_are_allowlisted_before_any_write)' --no-fail-fast`. Receipt `/tmp/surge-gate-authority-workspace-focused-final.log`, runtime result pending. Stronger comparison assertions remain pending until this run executes.


Final current-source workspace focused receipt `/tmp/surge-gate-authority-workspace-focused-final.log`: **exit0,24/24 passed**,4648 filtered/skipped,4.752s runtime. Strengthened historical gap context/criteria/proof/snapshot rollback assertions now executed successfully, along with core/projection/debug selector, actual Engine, original cold, marker0 variant, actual unwind settlement and mock allowlist. Workspace build retained a macOS linker compact-unwind `__eh_frame >16MB` warning for surge-ui and dependency future-incompat warnings (block/proc-macro-error2); no source/lint suppression added. Full same-union command `cargo nextest run --locked --workspace --no-fail-fast` running in `/tmp/surge-gate-authority-workspace-full-final.log`; before launch disk4.8GiB. Final gate environment remains explicitly local stripped-test evidence.


Pre-style-fix freeze997b1a full workspace receipt `/tmp/surge-gate-authority-workspace-full-final.log`: exit0,4644 passed/28 skipped,168.915s runtime. The28 skipped do not satisfy required ignored mock integration gates. Strict all-feature workspace first pass `/tmp/surge-gate-authority-workspace-clippy-all-final.log` exited101 on two new test-only style findings: type item after statements and `.ok().is_some_and`. Corrected mechanically with local type at function entry and `Result::is_ok_and`, no production changes/suppressions; initial build repair-review count remains0/3 (independent5a not started).

Latest final source manifest `/tmp/surge-gate-authority-source-manifest-final.sha256`:956 files, aggregate SHA256 **e1ef99870b2156d3bcc5ab86a59199711cb8db81712c9f9b31513dc741335150**. Previous manifests/receipts preserved separately.

Current final-freeze gates:
- `cargo clippy --locked --keep-going --workspace --all-targets --all-features -- -D warnings`: `/tmp/surge-gate-authority-workspace-clippy-all-final2.log`, exit0 (1m07s).
- `cargo clippy --locked --keep-going --workspace --all-targets -- -D warnings`: `/tmp/surge-gate-authority-workspace-clippy-default-final.log`, exit0 (1m23s).
- Same workspace focused filter as above: `/tmp/surge-gate-authority-workspace-focused-final2.log`, exit0,24/24 passed,4648 filtered/skipped,4.326s runtime.
- Same full workspace command: `/tmp/surge-gate-authority-workspace-full-final2.log`, pending. Disk before launch3.8GiB.

Remaining gates include doctests, exact local mock/restart ignored integration allowlist from justfile85–92/CI162–182 with freshly built actual helpers, actual release selector, final fmt/diff and manifest equality. Paid-provider ignored probes remain excluded.


Final-freeze full refresh `/tmp/surge-gate-authority-workspace-full-final2.log`: **exit0,4644 passed/28 skipped,178.382s runtime**. Workspace docs `cargo test --locked --workspace --doc`, `/tmp/surge-gate-authority-workspace-doc-final.log`: **exit0,6 passed/7 ignored**.

Required local-only ignored gates now executed with literal scoped targets from justfile/CI; all same final env, no paid-provider tests:
| Command | Receipt | Result |
|---|---|---|
| `cargo build --locked -p surge-acp --bin mock_acp_agent -p surge-cli --bin surge -p surge-daemon --bin surge-daemon` | `/tmp/surge-gate-authority-required-helpers-final.log` | exit0 |
| `cargo build --locked -p surge-mcp --example mock_mcp_server --features mock-server` | `/tmp/surge-gate-authority-required-mcp-helper-final.log` | exit0 |
| `cargo test --locked -p surge-acp --test bridge_rate_limit_classification --test reconnect_integration_test -- --ignored` | `/tmp/surge-gate-authority-required-acp-final.log` | exit0,2 passed |
| `cargo test --locked -p surge-orchestrator --test engine_e2e_linear_pipeline --test engine_concurrent_runs --test engine_resume_after_crash -- --ignored` | `/tmp/surge-gate-authority-required-engine-final.log` | exit0,3 passed |
| `cargo test --locked -p surge-mcp --features mock-server --test mcp_stdio_e2e -- --ignored` | `/tmp/surge-gate-authority-required-mcp-final.log` | exit0,3 passed |
| `cargo test --locked -p surge-daemon --test live_provider_smoke controlled_daemon_mcp_smoke -- --ignored --exact` | `/tmp/surge-gate-authority-required-daemon-final.log` | exit0,1 passed; settled=true/binding_verified=true |
| `cargo test --locked -p surge-cli --test daemon_restart -- --ignored` | `/tmp/surge-gate-authority-required-cli-final.log` | exit0,2 passed |

These11 actual allowlisted tests close the local mock/restart gate separately from the workspace28 skipped. Actual helper path/SHA256 bindings after successful builds: `/tmp/surge-gate-authority-helper-bindings-final.json`; legacy worktree target/debug alias still points to these shared outputs. No source edits since final freeze. Actual optimized release selector build pending (first invocation unqualified exact filter intentionally cannot count as runtime evidence; qualified exact invocation required afterwards).


## Builder handoff — BUILD COMPLETE

Implementation is frozen at final manifest **e1ef99870b2156d3bcc5ab86a59199711cb8db81712c9f9b31513dc741335150**,956 Rust/SQL files. `/tmp/surge-gate-authority-manifest-equality-final.json` confirms byte-for-byte manifest equality after all gates. No code pending from builder, no commits/push/PR/merge performed. Source changes are17 Rust/SQL files plus this task record: core run_state; persistence task_ledger and runs migrations/mod/reader/reader_views/storage/verification/writer, new projection and per-run0011/registry0033 SQL; orchestrator run_task and bootstrap validation integration test; daemon new wrapper/fixture; ACP test-only mock stage_mcp. No UI/Results/Stage B/Windows/Home/harness/receipt/sandbox source edits. Original fixtures remained untouched.

Final local gates all passed on this freeze:
- Focused workspace24/24 and full workspace4644/4644 (28 skipped separately identified), including UI; final2 receipts above.
- Strict workspace all-target Clippy, default and all-features, `-D warnings`; exit0 both.
- Workspace doctests6 passed/7 ignored; exit0.
- Required ignored local mock/restart allowlist11 unique tests passed (ACP2/Engine3/MCP3/controlled daemon1/CLI2). No live paid-provider probe.
- Actual optimized release selector: initial unqualified exact invocation `/tmp/surge-gate-authority-selector-release-final.log` exit0 after14m19s,0 tests, **build-only, not acceptance evidence**. Correct command `cargo test --locked --release -p surge-orchestrator --lib engine::run_task::tests::route_commit_selector_requires_exact_durable_occurrence_and_route -- --exact`, `/tmp/surge-gate-authority-selector-release-exact-final.log`: **exit0,1/1 passed**. Debug current-freeze focused test plus actual daemon code99 second-occurrence cuts passed; release selector returns false under actual optimized build. No extracted/synthetic selector or release-profile weakening.
- `cargo fmt --all -- --check`: `/tmp/surge-gate-authority-fmt-final.log`,exit0. `git diff --check`: `/tmp/surge-gate-authority-diff-final.log`,exit0.

Resource evidence remains separate from behavioral results. First release build stayed active exclusively in target/release while root audited60 inactive hashed debug test executables and freed3096MiB; `/tmp/surge-stageA-release-space-cleanup.json`. Libraries/standalone helpers/source/runtime/active unrelated orphan preserved, no source/profile changes. Earlier resource stops/ENOSPC receipts remain in this record and are not completed gates. Final disk5.0GiB; Cargo/rustc/linker inventory empty.

Helper provenance correction: initial `/tmp/surge-gate-authority-helper-bindings-final.json` is a pre-integration-build snapshot, not the hash of every later executed helper. Cargo test rebuilt standalone surge/surge-daemon under test profile; unchanged source, differing hashes. CLI restart selects `env!(CARGO_BIN_EXE_surge)` and its sibling surge-daemon; controlled smoke hosts real library Engine inside its actual test subprocess, selecting mock_acp_agent through target/debug alias and acp_smoke_proxy through current executable's shared debug/examples directory. Initial controlled smoke passed but proxy binding was not freshly confirmed. Subsequently `cargo build --locked -p surge-daemon --example acp_smoke_proxy`, `/tmp/surge-gate-authority-required-proxy-helper-final.log`,exit0, and fresh exact controlled smoke `/tmp/surge-gate-authority-required-daemon-bound-final.log`,exit0,1/1,settled=true/binding_verified=true. This final receipt closes actual fresh helper binding; old pass retained honestly. Post-all-build actual helper paths/hashes including proxy are `/tmp/surge-gate-authority-helper-bindings-post-build.json`.

Limitations/open owner steps: local debug tests used documented temporary symbols stripping/jobs2 for resource bounds, not literal CI-profile evidence. macOS UI compact-unwind linker warning and dependency future-incompat notices retained, no suppressions. Windows execution/CI and actual fresh isolated Sol61 native journey remain root-owned/unverified here; PR150 fourth Windows repair still unapplied. Independent Stage5a then5b remain required before overall acceptance, as does separately planned StageB closure. Builder does not self-certify those requirements.

**Cargo lease RELEASED** after all commands terminal and task-record handoff. Root owns next review/acceptance scheduling.


## Independent Stage 5a

Root dispatched `gate_authority_spec_review` after builder BUILD COMPLETE and explicit Cargo release. Independent local spec verdict **ACCEPTABLE**, no GA-S findings. Reviewer inspected complete tracked/untracked closure, genuine behavioral REDs and final executed receipts against the plan and historical contiguous-prefix amendment. Root independently rehashed all956 Rust/SQL entries with zero mismatches versus final e1ef998 manifest. This does not establish Windows/current CI or fresh native real-provider acceptance, does not close Stage B or the full goal, and is not merge-readiness. Stage5b generic/domain/security reviews are next. No source edits; review repair dispatch count remains **0/3**.


## Review repair 1 — recorded before dispatch

Reviewed revision: final e1ef99870b2156d3bcc5ab86a59199711cb8db81712c9f9b31513dc741335150 manifest,956 Rust/SQL files; source/destination same worktree/base/branch. Independent Stage5a ACCEPTABLE; Stage5b domain COMPLETE (no GA-D), security COMPLETE (no GA-A), generic REDO-TO-BAR.

- GA-Q-001 REQUIRED/SHAPE: new daemon fixture `tests/fixtures/bootstrap_gate_cold.rs:601–887` combines process driving/cut, replay downgrade, recovery/handoff and literal authority assertions. Extract coherent private phase/assertion helpers within this fixture. Preserve ownership: Host must remain retained across assertions and actual failure/unwind settlement, no earlier Drop/detached tasks, no weakened assertion or timeout.
- GA-Q-002 REQUIRED/BORROWING: same file834–840 deep-clones graph-bearing events only to form an iteration array. Iterate borrowed journal slices, preserving both literal observed prefixes and fullfold/cold-memory comparisons.
- GA-Q-003 ADVISORY: persistence boolean fixture scenario parameters are less readable; local rules permit these counts. Retained without change in this bounded repair: explicit literal test cases already exercise their authority decisions, and a new fixture vocabulary is not required to close the two blocking daemon concerns. No production/test-authority behavior change is authorized by this advisory.

Repair dispatch count **1/3** is recorded before implementation; shared across5a/5b. Sole Rust write zone is the daemon cold fixture; all production and persistence/orchestrator/mock files remain frozen. Dedicated original daemon fixture implementer performs this source-only repair; gate builder retains exclusive Cargo after source handoff. No Cargo while source writer is editing. Pure test refactor anchors existing real behavioral RED receipts; add no fabricated new RED claim.

After writer freeze: new source manifest/diff, actual full5-test daemon cold/unwind and affected integration regression execution, strict affected default/all-feature all-target lints/fmt/diff, current required gate refresh appropriate to changed fixture. Then independent affected5a BEFORE affected5b generic/domain/security check for preserved ownership/assertions. No commit until both review stages close. Home/harness3/3 and unapplied PR150 patch unaffected; StageB/C source still not started.


### Repair1 builder source inspection and destination freeze

Source-only daemon owner handed back `bootstrap_gate_cold.rs` frozen SHA256 **f987082c5cf49e0f3378c07e80b3f0cf62c3b61216df43708523a6908029babb**. Exact prior bytes `/tmp/surge-gate-cold-before-repair1.rs` independently match old e1ef manifest fixture entry461c11e1213d46e0e34d165426eb60b003fb2e775314ed15b697bb562945db35; repair diff `/tmp/surge-gate-cold-repair1.diff`. Builder independently inspected extraction: both Host values remain owned in run_in_home, helpers borrow them; explicit Drops, first settlement before SQL downgrade/reopen, fresh gate before preservation check, successful shutdown before final observations, and all literal authority/child/provider/assertion boundaries remain ordered. Authority helper iterates borrowed prefix and pre-terminal event slices; no graph-bearing Vec clone remains. No new behavioral RED claimed for mechanical test refactor.

Formatting unchanged writer hash. New deterministic destination manifest `/tmp/surge-gate-authority-source-manifest-repair1.sha256`:956 entries, aggregate SHA256 **3ee3e89d162b5889b1eb95f2942519247e0b2e1ff02c9648cf3c8d6679aeeb70**. Component equality receipt `/tmp/surge-gate-authority-repair1-component-delta.json` confirms exactly one changed fixture,955 unchanged files,zero removals. Production/persistence/orchestrator/mock/other helpers and release selector source remain identical to e1ef; GA-Q-003 advisory retained unchanged. Cargo scheduling resumed only after explicit owner handoff; initial disk5.6GiB. Repair count remains1/3.

Current-tree command env unchanged: exclusive shared target, incremental0/devdebug0/testdebug0, jobs2, temporary test-strip=symbols, login:false; same local-profile/Windows limitations. `SURGE_TEST_MOCK_ACP_BINARY` explicitly selects shared debug/mock_acp_agent. Agreed focused workspace filter (authority, distinct core names, projection, actual Engine, daemon5, debug selector, mock allowlist) running in `/tmp/surge-gate-authority-repair1-focused.log`; required final full workspace and strict both feature sets/fmt/diff follow. Previous e1ef full4644/requiredignored11/docs6/release1/boundmock1 receipts retained as prior executions, not claimed rerun; unchanged component hashes substantiate retaining unmodified docs/release/mock/restart dependency evidence.


### Repair1 builder handoff — BUILD COMPLETE, affected reviews pending

Frozen destination manifest **3ee3e89d162b5889b1eb95f2942519247e0b2e1ff02c9648cf3c8d6679aeeb70**,956 files. Only delta versus e1ef is daemon test fixture `bootstrap_gate_cold.rs`, SHA461c11→f987082c5cf49e0f3378c07e80b3f0cf62c3b61216df43708523a6908029babb;955 component hashes unchanged,zero removed. `/tmp/surge-gate-authority-repair1-manifest-equality.json` confirms freeze unchanged after all gates. No production behavior/source changes, no advisory-scope expansion, no commits/real-provider calls/original-runtime-fixture access/StageB/C/PR150 work.

Current destination receipts (same common env documented above, exact commands logged here):
| Command | Receipt | Terminal result |
|---|---|---|
| `cargo nextest run --locked --workspace -E 'test(graph_authority) or test(generated_child_cannot) or test(accepted_revision_restores) or test(runs::projection::tests) or test(route_commit_selector_requires_exact_durable_occurrence_and_route) or test(valid_flow_edit_requires_fresh_second_gate) or binary(bootstrap_gate_cold_recovery) or test(bootstrap_fixture_artifacts_are_allowlisted_before_any_write)' --no-fail-fast` | `/tmp/surge-gate-authority-repair1-focused.log` | exit0,24/24 passed,4648 filtered/skipped,4.381s |
| `cargo nextest run --locked --workspace --no-fail-fast` | `/tmp/surge-gate-authority-repair1-workspace-full.log` | exit0,4644 passed/28 skipped,163.174s |
| `cargo clippy --locked --keep-going --workspace --all-targets --all-features -- -D warnings` | `/tmp/surge-gate-authority-repair1-clippy-all.log` | exit0,8.54s |
| `cargo clippy --locked --keep-going --workspace --all-targets -- -D warnings` | `/tmp/surge-gate-authority-repair1-clippy-default.log` | exit0,6.88s |
| `cargo fmt --all -- --check` | `/tmp/surge-gate-authority-repair1-fmt.log` | exit0 |
| `git diff --check` | `/tmp/surge-gate-authority-repair1-diff-check.log` | exit0 |

All five authentic daemon cold/unwind tests and actual Engine edit/regenerate gate regression executed in focused and full destination gates. Original production RED evidence remains the behavioral anchor; no new RED fabricated for this pure test-shape repair. Only prior known UI compact-unwind linker and dependency future-incompat notices observed; no suppressions or new findings. Local strip-profile/Windows limitations unchanged.

Prior e1ef requiredignored11/docs6/release1/boundmock1 receipts are preserved as prior executions, **not rerun in repair1**. The changed fixture is consumed only by daemon bootstrap_gate_cold_recovery; component-delta receipt establishes byte-identical production/release selector/docs/mock/other test helpers and dependency closure for retained evidence. Full workspace was freshly rerun as required project gate after review repair. Root's affected Stage5a must precede affected Stage5b; count1/3 remains unchanged. Advisory GA-Q-003 unchanged.

Every Cargo command is terminal; Cargo/rustc/linker process inventory empty, final disk3.3GiB. **Cargo lease explicitly RELEASED** to root for affected independent review. Builder freezes source and performs no further edits/gates without a concrete finding.


## Root local stage closure after independent repair1 review

Affected Stage5a gate_authority_spec_review ACCEPTABLE/no GA-S preceded affected Stage5b. Generic gate_authority_quality_review COMPLETE: GA-Q-001/002 resolved; GA-Q-003 retained advisory without expansion. Domain gate_authority_domain_review COMPLETE/no GA-D; security gate_authority_security_review COMPLETE/no GA-A. Reviewers checked the preimage/diff/current fixture and execution receipts; all59 assertions, timeouts, coordinator-owned Hosts, settlement/Drop order and unwind behavior remain. No new blocking findings. Shared repair dispatch count1/3.

Root independently rehashed all956 destination Rust/SQL entries: manifest3ee3e89d162b5889b1eb95f2942519247e0b2e1ff02c9648cf3c8d6679aeeb70, zero mismatches. Current focused24/full4644 (28 skipped), strict default/all-feature workspace all-target Clippy, fmt/diff checks passed. Earlier docs6/requiredignored11/release1/boundmock1 are retained unchanged-component evidence, not represented as rerun. Genuine behavioral REDs are recorded above; pure test refactor introduces no fabricated RED.

Local implementation stage COMPLETE, full18-file dependency closure ready for authorized local commit. Windows/current CI, fresh isolated native codex-acp/gpt-6.1-sol journey, StageB visibility and StageC same-task planning remain unverified/open. No push/PR/merge authority is inferred for this independent branch. PR150/Home fourth repair remains unapplied and requires the unanswered explicit revised mandate; no memory or original runtime fixtures changed.
