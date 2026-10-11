# Durable settlement implementation stages

Status: documentation-only proposed execution decomposition of accepted durable-settlement-plan.md SHA-256 `9b509500aa927a3368c0ebfe2d931c71fca035ea6c08f34ce5a365d0e789ac54`. Full accepted scope remains required. Existing engine review/repair counter remains1/3. No schema/Rust writes or Cargo execution authorized by this document; root opens next source window after prerequisite evidence and assigns sole build token.

## Prerequisite evidence and ownership

Root reports actual daemon RED3: real Engine original outcome Completed, sole failed ForwarderRead after sealed prefix4, normal confirmed-control PASS; durable_outcome still reconstructs success, terminal reconciliation emits Completed/outbox1, actual merge mock records merge_calls1/marker1. This document records root's report, not a newly executed test or invented raw log path. Retain exact RED source/log/receipts under existing history-bypass evidence before repair. CLI historical bypass RED remains pending; finish it before implementation cutover. Do not replace those real-owner regressions with hand-created RunCompleted fixtures or implementation status flags.

Root owns sequencing, source freezes, shared-file integration, build token and local stage commits. Assign one persistence owner for all opening/lease/registry/migration/namespace changes; one orchestrator owner for actual execution publisher/fork/revalidation; one consumer owner for daemon/CLI/UI/outbox. These are proposed work ownership boundaries, not authorization to spawn agents or concurrent shared-file edits. Coordinate scopes before edits. Independent architecture/spec reviewer must differ from implementers; existing repair count is not reset per stage.

Stages below are dependency-closed local checkpoints, NOT individually deployable acceptance or a scope reduction. No intermediate stage may be published as a complete fix while old history bypass/delete races remain. Each commit includes full dependency closure and actual checks run, with outstanding requirements retained. No push/merge authority inferred.

## Stage A — additive proof storage and every opening path

Depends: retained daemon+CLI RED and next root source window. Owner: persistence; orchestrator owner supplies fork call-site closure in SAME checkpoint.

Owning files: runs/{storage,registry,registry_exec,writer_slot,file_lock,reader,pool,connection,migrations}.rs and registry migration directory; state_home internal read-only identity/creation preparation APIs; work_items/owned_flow/refusal_delivery.rs; engine/fork.rs; narrow new runs/host_settlement module(s) if useful. Update AGENTS only for meaningful new module/entry structure.

Implement additive tables/types for exact run opening, immutable settlement, unresolved Engine obligation, permanent deletion intent and immutable inventory header/rows (schema supports later deletion stage; no fabricated successful backfill). Implement versioned held platform identity and independent fixed byte vectors for exact schema_version-aware journal row framing. Centralize original token+OS lock+opening capability acquisition. Every existing writer and direct refusal JournalOwner participates before journal mutation/migration; no provider recipe epoch reuse.

Implement cancellation-fenced directory/artifact/empty-lock-only preparation transaction and opaque RunCreationReservation. Regular create_run and actual fork reserve/copy/create handoff consume SAME capability/epoch/guards. No arbitrary existing-directory adoption. Read managers initial/replacement connections become existing-only/read-only, no CREATE, with compatible initialization and native manager fences; preserve public subscription lifetime contract. Tombstone admission present in ALL initial/pre-lock/under-lock paths from outset.

Tests/checks: actual same-RunId creator collision; creator paused across tombstone transaction; barrier committed before migration/first append; failed/cancelled open invalidates older proof; direct informational writer invalidates too; fork successful copy+handoff does not double-create/acquire; fork cancellation real copy worker retains reservation; public reader existence-check/reconnect after removal never recreates; supported WAL/SQLite read-only polling and public subscribe-outlives-storage remain green. Exact framing vectors cover stored schema_version conversion/migration/kind mismatch and identity ordering. Focused persistence/fork tests, root-owned appropriate cross-target compile/lint. If WAL/native-manager policy cannot coexist, return precise design issue; no CREATE fallback.

Stage-A completion does not switch consumers to a fail-closed shim. It installs real authority plumbing required for subsequent normal receipts and explicit legacy recovery; existing bypass regressions remain intentionally open until complete cutover.

## Stage B — actual joined publisher and read authorization

Depends: A plus existing actual writer seal/forwarder settlement implementation. Owners: persistence+orchestrator with explicit file partition.

Owning files: runs/{run_writer,writer,reader,error,host_settlement}.rs; engine/{engine,run_task,run_forwarder,handle,error}.rs. Implement Engine mode before effects/feed obligations; publisher retains SAME original WriterLease through actor join, actual SQL/forwarder joins, dropping all execution-owned readers and final snapshot verification, then cancellation-fenced registry commit actual worker join. Scope excludes independent subscribers/unrelated resources. Receipt+join failures preserve original outcome as SettlementFailed; hard abort remains UNKNOWN/nonfatal.

Implement existing-only ConfirmedRunHistory acquisition/guard and exact one-snapshot verification: identity, latest epoch/mode/unresolved obligation, schema/version/kind, complete contiguous rows, MAX/count==sealed and no suffix including zero. Publication must not infer proof from registry RunStatus or terminal events. Narrow guard gives no append/receipt synthesis authority.

Tests/checks: genuine actual SQL gate keeps normal completion Pending; actor/feed/publisher worker gates retain original exclusion after caller abort; final drain Busy until proof commit+actual release; replacement opening supersedes proof; actual suffix/schema-version/kind/DB replacement invalidates; normal completed/failed/aborted/suspended/parked outcomes preserve existing behavior only after real joins; publication failure never returns success and consumer guard holds same OS exclusion cross-process. Replay daemon RED's normal-control path now has real valid receipt; failed-forwarder path remains unconfirmed. Focused actual ownership tests+strict checks under root token.

## Stage C — existing explicit historical recovery capability

Depends: A+B. Must exist BEFORE consumer cutover so missing/legacy receipts retain legitimate explicit recovery rather than permanent fail-closed functionality loss. Owners: orchestrator+daemon/CLI dispatcher owner.

Owning files: existing authenticated/local explicit resume/recovery dispatchers and facade parameter seam; engine terminal-resume path; runs/host_settlement revalidation publication. Inventory actual dispatcher call graph first; no new CLI/IPC/approval surface. Private HistoryRevalidationAuthority originates ONLY trusted existing explicit dispatch; bound exact run/current opening/command operation and consumed once via CAS. Shared automatic wake/capacity/startup resume carries None. Existing valid JoinedExecution consumes automatically without new opening/reverification requirement.

Explicit terminal legacy/UNKNOWN resume performs fresh finite seal/drain and exact snapshot joins, publishes distinct ExplicitHistoryRevalidation, preserves audit original UNKNOWN and existing revision/verification/automation/user-approval policies. JournalOnly close cannot clear an unresolved Engine obligation or rebind an old execution receipt. Receipt grants no new merge permission and imposes no blanket new approval workflow.

Tests/checks: actual same facade invoked by automatic wake/recovery cannot mint proof; existing explicit operator command succeeds through true fresh verification; wrong-run/stale/reused capability refuses; legacy terminal history readable but not automatic success absent proof; existing explicit recovery restores intended completed-history functionality; fresh valid JoinedExecution automatic recovery unchanged. Meaningful decoder/capability controls use existing production dispatcher, not a public bool bypass.

## Stage D — all proof consumers and actual outbox delivery

Depends: A+B+C. Owner: consumer owner; root resolves overlaps with stale task-reconciliation work. This is one behavior cutover checkpoint with all actual action-authorizing consumers, not a partial daemon-only fix.

Owning files: daemon intake_completion, automation_merge_gate, tracked_run/work_items terminal reconciliation, inbox/recovery consumers; CLI engine::require_completed_history/bootstrap/run_lifecycle; relevant facade/UI history-derived success; existing durable external-effect/outbox intent schema/mappings. Keep pure history display separate from authorization.

Replace journal-only outcome reconstruction at actual actions with ConfirmedRunHistory. Exact receipt identity/provenance/journal prefix plus existing revision/verification/automation/approval references bind local outbox/decision commit under guard. Actual later delivery reacquires/revalidates ALL bound authority; actual RPC task owns guard through result even if caller cancellation occurs. No SQLite transaction across RPC. Existing MergeAttempted/uncertain/idempotent delivery rules remain; timeout never means retry authority. New writer blocked during actual guarded delivery. No bare run ID/bool survives guard as action permission.

Tests/checks: retained daemon RED3 must now prevent success comment/outbox/merge for sole failed-forwarder outcome, using actual production Engine failure owner; CLI actual history RED becomes rejection without proof; normal valid receipt control still emits exactly intended success/actions once. Startup/periodic/lag replay all honor same boundary. Gate between outbox commit and delivery, open replacement writer, prove stale intent cannot dispatch; gate actual RPC, prove writer/delete Busy until actual delivery ends; cancelled caller cannot release actual owner early; repeated delivery preserves existing dedup and uncertain no-replay. Explicit legacy recovery control uses C and existing permissions, not test-only receipt injection.

## Stage E — durable deletion inventory and crash continuation

Depends: A+B; integrate before declaring D/final release complete. Owner: persistence; no production delete command exists. Preserve trusted explicit PUBLIC Storage::delete_run library invocation as administrative boundary; add no new product surface.

Owning files: runs/storage::delete_run, host_settlement inventory/cleanup helper, additive migration fields if A needs reviewed correction, internal descriptor observation/removal primitives. Initial delete acquires original OS exclusion, captures COMPLETE bounded immutable inventory and outside-parent identity/security, commits inventory+tombstone+receipt invalidation atomically BEFORE removal. Retry begins registry cleanup gate FIRST, verifies persisted header/count/byte bound/digest, matches every remaining original entry, permits missing originals only as committed deletion progress, never creates/acquires missing lock or adopts new identities. Actual cleanup worker holds SECOND registry IMMEDIATE transaction and outside-parent fence; after revalidation releases in-tree native locks only there, performs exact bounded cleanup, records Deleted before gate release. First tombstone transaction never rolls back with cleanup bookkeeping.

Freeze exact inventory row framing and outside-parent snapshot BEFORE implementation source freeze as required by independent review; root is outside per-entry inventory and must be durably bound separately. Limits100000 entries/depth64/64MiB TOTAL encoded inventory, raw Unix/UTF16LE Windows paths, no recursive fallback. OS nonreturn preserves actual owner/UNKNOWN; no deadline steals cleanup. Inventory immutable/no cascade deletion/no RunId reuse.

Tests/checks: cross-Storage/process confirmed/delivery guard makes initial delete Busy; delayed creator and reader reconnect cannot recreate tombstoned tree; actual cleanup child crashes after regular file, original lock and root removals before bookkeeping commit; independent reopened Storage trusted API retry proves same persisted inventory, no missing-lock recreation, replaced/extra node refuses unchanged, corrupt/incomplete/over-limit inventory refuses. Concurrent retries serialize actual gate; cancelled API caller retains actual cleanup owner; outside-parent replacement/security change refuses. Explicit no-tombstone initial deletion with missing original lock fails safely.

## Stage F — integrated evidence, independent reviews and native closure

Depends: A–E all complete. Root retains complete accepted manifest and checks every consumer/writer/create/delete entry closure. Run focused integration acceptance, required workspace checks under one token, then DIFFERENT independent spec/security and quality reviews against exact frozen source+accepted plan. Record command receipts/source hashes and unresolved requirements; local checkpoint commits stage-sized, never mask missing behavior with test ignores/suppression.

Native Windows cross-process ownership, loader/provisioner actor acceptance, teardown/fault matrix and real remaining Windows CI regressions retain their separate required evidence. Local/macOS or compile-only tests do not close Stage1 or5a/5b. Existing engine counter1/3 and Stage1 counter3/3 remain unchanged. Final release claims require actual checked native commands and full dependency closure, not staged schema, plan acceptance, synthetic abort flags or source-only confidence.

## Critical path and stop rules

Critical path: prerequisite daemon+CLI RED -> root source window -> A -> B -> C -> D, with E after A+B and integrated before F. Deletion may be implemented after D as a local checkpoint only; no release occurs before E. Authors can prepare tests/documentation while waiting, but no Rust/schema mutations before root grants window and no competing Cargo processes. A bounded failing stage returns its precise unresolved design/ownership issue to root; do not add fallback, automatic legacy proof, global repair/privilege, arbitrary recursive deletion, consumer-only fail-closed shim, or reset repair count to proceed.
