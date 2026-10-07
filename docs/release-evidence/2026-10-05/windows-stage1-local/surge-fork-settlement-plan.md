# Fork writer settlement and protected child reservation

Status: owning lead/scout proposal, ACCEPTABLE for independent critic/security
review; no production edits or Cargo commands executed. Native behavior and new
RED oracles remain unverified. Scope is completed fork futures and their returned
success/error, not a new cancellation supervisor.

## Evidence and concrete contract

Current engine/fork.rs acquires parent_writer before these fallible operations:
raw create_dir, artifact inheritance, checkpoint restoration, child create_run,
child append, child snapshot, child close and parent lineage append. `?` at any
of those sites can bypass parent_writer.close. Child append/snapshot errors also
bypass child_writer.close. RunWriter::Drop merely enqueues Shutdown and does not
await its join; its actor shares the actual WriterLease. Therefore a returned
error need not mean the original writer slot/file lock is already released.

Storage::open_run_writer first tries ActiveWriters::try_acquire; that future has
no suspension in the uncontended mutex path. An immediate reopen is a useful real
ownership oracle. The existing telemetry regression demonstrates a single-worker
multi-thread Tokio runtime plus a spawned test body (not the outer block_on
thread) can expose fire-and-forget return as WriterAlreadyHeld without sleeps.

Windows raw create_dir additionally produces an inherited runtime run directory,
where the new backend requires protected current-user SQLite directory creation.
Backend is separately refining RuntimeHomeOwner::reserve_run_directory(RunId):
exclusive create of the child run and its artifacts directory, retained original
directory ownership, no SQL connection/DB fence, and refusal on any existing child.
This plan consumes that accepted capability; it does not implement another ACL
or reservation mechanism in orchestrator.

Required behavior:
- A returned fork result has awaited close/join for every writer it acquired.
- A child is fully written and closed before parent ForkCreated is appended.
- Child write/snapshot/close failure produces no parent lineage.
- Parent close is attempted even after reservation/copy/create/child/lineage errors.
- If operation and close both fail, preserve both typed failures; do not overwrite
  the original operation error or silently discard close failure.
- Existing reservation collisions and corrupt artifact failures preserve original
  bytes/index and parent history. No automatic deletion of partially created child
  runs/checkouts is introduced: existing cross-database non-atomic semantics stay.
- Windows retains the protected run reservation through artifact copy, checkpoint
  restoration and Storage handoff; Unix reservation semantics remain unchanged.

## Minimal implementation shape

Files: engine/fork.rs and engine/error.rs, plus their adjacent tests. Backend owns
the reservation API and its independent creation/collision/native tests.

1. Keep pure/read-only validations before opening any writer: task exclusion,
   prefix bounds/start, graph edits, snapshot/checkpoint validation.
2. Acquire parent_writer in fork. Evaluate an inner async operation borrowing it;
   this body performs reservation, copy, child handling and parent lineage.
   Capture its Result instead of returning via `?` from the outer writer-owning
   scope. After the operation resolves, always await parent_writer.close().
3. Factor child history persistence into a private natural helper that owns its
   real RunWriter, copies events and optional snapshot, captures that result,
   then always awaits child close. Use one shared result-combination function for
   both parent and child settlement. No generic trait mocking the writer, new
   executor task, or hand-written duplicate writer implementation is needed.
4. Child operation success includes successful close. Append ForkCreated only
   after the helper returns success; the outer parent settlement still executes
   if lineage append fails.
5. On Windows, obtain the existing runtime-home capability through the accepted
   public API, then reserve_run_directory(req.new_run). Retain the returned
   directory owner in the operation's scope until child create/write/close has
   handed off and settled. No std/tokio create_dir on that Windows route. Map
   reservation failure to the existing ForkInvalid category; preserve genuine
   collision and access/security distinctions in its diagnostic, without ACL
   repair. Unix keeps its original exclusive tokio create_dir operation.

The existing telemetry append_bootstrap_telemetry uses precisely the four-way
(write,close) combination. Reuse that pattern, not its error enum: fork's operation
can fail with any EngineError, including a nested child settlement failure.

Suggested additive EngineError variants:

    ForkWriterClose {
        run: RunId,
        #[source] close: Box<surge_persistence::runs::CloseError>,
    }
    ForkOperationAndClose {
        run: RunId,
        #[source] operation: Box<EngineError>,
        close: Box<surge_persistence::runs::CloseError>,
    }

Box the nested errors to avoid inflating the widely used EngineError. Display
includes operation plus close and the affected run id. The source chain selects
the original operation when both exist; callers can inspect the explicit close
field. Ordinary operation failure with successful close returns the original
EngineError unchanged. A child combined error can be nested as the parent's
operation error, preserving all three failures if parent close also fails.

This is an additive public enum change in an active-development workspace; audit
exhaustive matches across crates. Do not introduce a new public writer-role enum
when the already available RunId identifies the failed settlement. Trace the close
failure with stable event/run fields if needed; do not log raw event payloads,
artifact contents, credentials or workspace paths to diagnose ownership.

Cancellation limitation: dropping the fork future while it is executing still
uses RunWriter's established cancellation fallback and retained actor lease. This
bounded fix guarantees settlement before a returned Result; it does not falsely
claim a synchronous destructor can await, or silently launch an uncancellable
fork after caller cancellation. Extending cancellation semantics requires a
separate owning design and is not hidden in this patch.

## RED-first oracles before behavior change

All fixtures retain the protected home and explicitly close created writers.
Tests run their full body in tokio::spawn under multi_thread, worker_threads=1,
then immediately reopen writer(s) following fork/helper return. No sleep, retry,
metadata-existence proxy, or arbitrary larger timeout. Outer deadline only bounds
hung-test failure. Actual output must show WriterAlreadyHeld (or the exact real
ownership failure) before claiming RED; compilation failure is not that proof.

A. Existing child collision: seed a parent, create the occupied child directory
and sentinel through an appropriate real path/capability, record bytes/history,
call public fork, assert the collision category and unchanged state, then reopen
parent immediately and close it. Strengthen the current reused-destination test
or add a focused fixture without replacing its independent artifact assertions.

B. Corrupt inherited artifact: retain current real hash mismatch test. Immediately
reopen parent after returned hash mismatch, before reader awaits or other yields.
Check no lineage and no copied corrupt artifact. This exercises a different early
error after reservation, not merely the same collision branch twice.

C. Parent lineage write error: install a real SQLite BEFORE INSERT trigger on the
parent events table rejecting kind ForkCreated. Call public fork; child completion
must be intact, parent lineage absent, and parent/child writers immediately
reacquirable. This uses actual SQLite failure, not a synthetic closure result.

D. Child append and snapshot errors: no unsafe race or public test hook is needed.
First extract the private child-history helper with the current behavior unchanged
(`?` before close). This is a semantic-preserving precursor, not the fix. Seed a
real child DB/writer and install a real SQLite rejecting trigger before invoking
that exact production helper. One trigger rejects an appended event; another
rejects graph_snapshots insertion after a valid prefix (writer.rs currently uses
INSERT OR REPLACE INTO graph_snapshots). Immediately reopen the child
on returned error. Record genuine runtime RED, then implement always-close in the
same production helper. Recheck table/trigger names from actual migrations before
writing either fixture. Exercise this helper in public fork normal/error tests so
it is not an unused test-only imitation.

E. Four-way settlement result mapping: ordinary value retained; original operation
error unchanged on successful close; close-only typed variant includes run; combined
variant retains exact original and close errors with correct source/display.
Include nested child-plus-parent close failures. Synthetic CloseError values here
are explicitly tests of deterministic error composition, not proof of an actual
OS/SQLite close fault. Actual close/join is proved by A-D and the successful path.

F. Normal fork: immediately reacquire both writers after success; verify prefix,
snapshot, artifact paths/content and exactly one parent ForkCreated. Existing
success/edit/worktree tests remain required.

G. Windows reservation: execute public fork under normal and dedicated standard
user tokens; independently inspect actual run/artifacts ACLs and current-user
owner. Existing-child collision leaves ACL, artifacts/index and sentinel bytes
unchanged. Fixture setup success alone is not native reservation acceptance.
Backend capability oracles cover no-SQL reservation and lifetime through handoff.

If the scheduler-based RED does not reproduce, stop calling it proven and derive a
real synchronization boundary from writer ownership rather than add retries or
ship an unexecuted claim. Existing telemetry technique is evidence-backed precedent,
not a guarantee that these new tests have already failed.

## Gates and boundaries

Independent critic plus owning security/async review before production edits;
then RED receipts, implementation, targeted fork/telemetry/fixture suites, strict
orchestrator all-target/all-feature Clippy, formatter and affected cross-crate API
check. Full Windows tests plus dedicated standard-user native reservation/cleanup
are necessary for the Windows routing claim. CARGO_INCREMENTAL=0, disk and single
Cargo token constraints remain in force. No Cargo was run in this scout task.

Spec review precedes quality/unsafe/security review. No tests disabled, no cleanup
failure ignored, no arbitrary retry, no no-op alias capability or raw mkdir fallback.
