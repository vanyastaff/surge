# Daemon maintenance and inbox settlement ownership

Status: revised after independent RESHAPE findings; owning-role maintainer verdict ACCEPTABLE for this revised contract; independent re-review required before code. Read-only discovery completed. No Rust edits or Cargo checks performed for this slice.

## Scope and observed defects

- `crates/surge-daemon/src/server.rs:175` starts reconciliation before fallible socket setup and discards its handle. A bind/name/publication error leaves a task holding `TrackingContext`.
- `server.rs:205` detaches the drain loop; successful server return currently joins only connections.
- `crates/surge-daemon/src/inbox/consumer.rs:145` detaches every `TicketStateSync`.
- `crates/surge-daemon/src/inbox/state_sync.rs:48–75` can return after terminal event handling while dropping an unfinished `RunHandle.completion`.
- `crates/surge-daemon/src/main.rs:1533` detaches the consumer itself; a fix only inside the consumer would not reach production graceful shutdown.

## Implementation

### 1. Server maintenance: inline owned futures

File: `crates/surge-daemon/src/server.rs`.

Complete socket preparation, bind and publication before starting any maintenance. Replace `spawn_task_reconciliation` and `spawn_drain_task` with ordinary async loops. Drive both concurrently with the existing connection-serving loop using `tokio::join!`.

Successful server return means connection handlers and both maintenance loops finished. At each maintenance loop boundary, use cancellation-first biased selection: if cancellation and a timer/admission wake are both ready, cancellation wins. Token cancellation stops each maintenance loop at its existing operation boundary: an in-flight reconciliation page or start completes normally, and no next timer tick/page begins afterward. Check the token again after a selected wake and before entering the operation, so cancellation observed at that boundary cannot start another page. A currently running reconciliation page retains its existing semantics; this slice does not claim to add a new per-attempt shutdown admission fence. Existing productive run tracking remains under current admission/broadcast ownership.

Inline maintenance futures borrow dependencies instead of creating detached owners. No new public API or dependency is required. A panic now propagates through the server task and its existing outer join diagnostics instead of disappearing in a detached maintenance task.

### 2. Consumer: own all accepted ticket followers

File: `crates/surge-daemon/src/inbox/consumer.rs`.

Split `run` into two concurrently driven private futures:

1. The existing polling/action loop hands successfully launched `LaunchedRun` values through a bounded channel with capacity one.
2. A follower driver receives launches and owns their `TicketStateSync::run` futures in `FuturesUnordered<BoxFuture<'static, ()>>`.

Drive both with `tokio::join!`; do not spawn child follower tasks. At each poll/action boundary prioritize cancellation over a ready interval or next queued action. Finish the actual in-flight action and its successful launch handoff and cursor update; do not select cancellation against that in-flight operation or discard its returned RunHandle. Then close the producer and drain all accepted followers. A cancellation check before the next action prevents processing the rest of a previously fetched pending batch after shutdown has been observed. Preserve action failure logging and cursor advancement semantics. Channel backpressure must never discard a launched run. The receiver continues to service handoffs while polling every active follower.

This uses existing `futures` and Tokio dependencies. It avoids detached followers and avoids treating abort-on-drop JoinSet cancellation scheduling as synchronous settlement.

### 3. Ticket sync: retain and join actual run completion

File: `crates/surge-daemon/src/inbox/state_sync.rs`.

Poll the retained `RunHandle.completion` concurrently with event reception; do not put completion joining behind a possibly never-closing event stream. A retained broadcast sender combined with failed completion must terminate the follower, not hang it.

The follower keeps explicit event-observation state and never drops/replaces the completion handle before resolving it during cooperative operation:

- Persisted events continue existing Active-state handling.
- A terminal event performs the existing ticket/outbox update once, records that observed outcome and disables further event handling, but does not return until completion resolves.
- A successful completion arriving before a terminal event supplies the actual joined RunOutcome for the existing terminal handler, once. A completion arriving after a terminal event is compared with the recorded outcome; disagreement is logged explicitly, not silently converted to success or a second conflicting terminal transaction.
- A failed completion logs the join failure and terminates even when the broadcast sender is still retained. If no terminal evidence was already handled, a bounded-by-existing-storage-operation durable outcome inspection may recover a persisted result; absence or inspection failure does not fabricate a terminal ticket state.
- A closed event stream disables event reception and may inspect existing durable terminal history, while retaining/concurrently awaiting completion. A lagged stream keeps the existing durable fallback. Any recovered terminal result is handled at most once. No closed-channel hot loop.

A ticket reaching Completed is not engine settlement. On hard parent abort, dropping the follower also drops its retained JoinHandle; Tokio then leaves the engine task running. This is an explicit unconfirmed state, not evidence of an engine join or successful cleanup. Do not add an abort-on-drop engine policy here: productive runs retain their existing execution semantics, and final HostRuntime ownership remains responsible for teardown after grace expiry.

### 4. Production handle reaches daemon grace handling

File: `crates/surge-daemon/src/main.rs`, limited to consumer handle plumbing.

Return `JoinHandle<()>` for the consumer from `spawn_inbox_subsystems`, including the Telegram-disabled early-return branch. Retain it at the caller. Both server and consumer spawned tasks must carry an owned cancel-on-exit guard constructed before spawning, so even cancellation before first poll, an unexpected early return, or a panic cancels the shared shutdown token. The guard logs an unexpected exit only when the shared token was not already canceled, then cancels it. Expected token-driven completion produces no false failure diagnostic. This is required because `lifecycle::drain_until` waits for cancellation before inspecting task completion; merely adding `is_finished()` cannot prevent a pre-shutdown panic deadlock. Preserve and report the actual JoinError/result after drain. Include the consumer's `is_finished()` status in the existing grace predicate and await it after draining. On grace expiry, explicitly log unfinished inbox settlement, abort and await the consumer under the existing policy.

Do not rewrite unrelated scheduler, Telegram, intake or process shutdown ownership. Do not claim consumer abort joined its engine completion task. Keep the coordinator's existing final `HostRuntime` teardown responsibility explicit in code/comments and evidence.

## Cancellation and ownership contract

Cooperative token shutdown followed by successful joins settles the work owned by this slice. Active runs may finish during grace. Canceling an inbox consumer must not silently abort their execution.

Hard abort is not completion evidence. `work_items::reconcile_page` starts a blocking SQL closure around line 850. Already-running `spawn_blocking` work cannot be synchronously canceled by dropping a parent future. Neither JoinSet nor FuturesUnordered changes that fact. The existing `HostRuntime` teardown remains the final production owner after grace expiry. Do not claim forced server/consumer cancellation proves engine, SQL, external process or complete daemon settlement.

Tests needing immediate fixture cleanup must establish successful cooperative joins. No cleanup sleeps, retry-until-unlocked workaround, or test-only Runtime drop substitutes for ownership.

## Observable acceptance and actual RED oracles

1. **Startup failure leaves no maintenance owner.** Use a valid Tokio MultiThread runtime with exactly one worker; `Storage::open` rejects CurrentThread runtimes. Prepare real Storage/Engine/TrackingContext, then run the no-yield assertion body in a task on that sole worker. Call `run_runs_only` with a deliberately invalid socket name and compare retained Arc ownership immediately after its synchronous error path returns, without an intervening yield/sleep. Baseline starts reconciliation first and leaves its cloned TrackingContext in an unpolled spawned future on the same worker, so this deterministically fails. No background engine runs are started by the fixture.
2. **Successful shutdown joins maintenance.** Use the same valid single-worker MultiThread runtime and execute the critical assertion body on its sole worker. Use an already-canceled token and valid private socket path. After successful server return, without intervening yields, assert original real storage/facade ownership counts are restored. Baseline spawns reconciliation and drain futures, then returns before either executes. Account explicitly for all fixture-owned clones rather than assuming an arbitrary universal count. The cancellation-first design must also establish that a simultaneously ready first interval does not run a page.
3. **Consumer waits for the accepted execution's real completion.** Extend the existing inbox integration facade with explicit channels. Emit real terminal notification, hold completion behind a oneshot, and retain a storage clone in that completion future. Pin and drive the actual `consumer.run(...)` future directly. Establish the handoff by observing the real processed-action receipt and terminal ticket/outbox update while driving that future; the mock facade returning a handle alone is not handoff acknowledgment. Then cancel and directly poll the already-driven consumer future: it must remain Pending while the completion oneshot is held. Do not use a spawned consumer JoinHandle's Pending result as the oracle, because that may mean only that the task has not been scheduled. Release completion; await the same consumer future; assert ticket/outbox state and owner release. Baseline returns Ready prematurely. Use handshakes/observable receipts, not sleeps; a timeout is only the outer anti-hang bound.
4. **Completion and shutdown edge paths.** Retain an actual broadcast sender while the actual completion task fails: the follower must return with the failure observed, even though the event stream is open. Also cover completion success preceding the terminal event, terminal event preceding delayed completion, and closed events with pending completion. An empty consumer exits on cancellation; multiple accepted followers all drain. Cancellation concurrent with a ready tick or next action must win at the boundary; an already-running launch must finish its handoff before shutdown drain. Do not infer failure observation merely from channel closure.
5. **Unexpected task exit begins shutdown.** Exercise the same production cancel-on-exit owner with a panicking server/consumer future and with early normal return, starting with an uncanceled token. Assert cancellation is triggered and existing drain completes without first waiting for an external signal. Exercise expected token-driven exit separately. Include cancellation before the spawned task's first poll, since its guard must already be owned then.
6. **Existing behavior survives.** Run selected queue-drain, bootstrap, inbox callback and graceful-shutdown regressions. Native Windows fixture cleanup after successful joins adds ownership evidence; a passing macOS count oracle alone is not native Windows proof.

Write these tests first and obtain actual failing command receipts before behavior changes. Parent controls the exclusive Cargo token. No Cargo commands until explicitly released.

## Write zone and gates

Production write zone: `server.rs`, `inbox/consumer.rs`, `inbox/state_sync.rs`, and the small `main.rs` consumer-handle/grace plumbing. Tests remain narrowly associated daemon tests; coordinate existing fixture-owner edits before writing.

No commits or pushes by the builder. Use CARGO_INCREMENTAL=0. Run the project-scoped nextest regressions, daemon strict Clippy with the repository feature set, and formatting. Independent spec review precedes code-quality review. Preserve command receipts and distinguish cooperative settlement from hard-abort limitations.

## Maintainer assessment

ACCEPTABLE as a bounded daemon ownership correction: ownership remains in the existing crate, existing asynchronous primitives suffice, no compatibility shim/public API/dependency is introduced, and active-run semantics are preserved. Inline futures provide concrete lifetime ownership instead of hiding detached work behind fixture timing. The production main caller is included. The independent reshape findings are explicitly incorporated: concurrent completion/event observation, cancel-on-unexpected-exit ownership for both production tasks, valid MultiThread RED fixtures, cancellation-first operation boundaries, and direct actual-future polling for the consumer RED oracle. Remaining review risks are handoff/cursor linearization, bounded-channel drain liveness, duplicate/conflicting terminal evidence, and avoiding false completion claims after grace expiry.
